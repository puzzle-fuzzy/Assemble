//! 接收流水线对着真实的服务端跑一遍：真的发文件、真的落盘、真的回执。
//!
//! 桌面壳里只有这里验证的是"整条路"——事件、下载、核对、落盘、通知、回执、重试。
//! 服务端是真实的实现（跑在回环地址上）；应用用的是 Tauri 的模拟应用：
//! 没有窗口，但状态、任务与网络走的都是真实运行时的那几段代码。
//! 唯独事件流那一环（[`crate::runtime`] 把事件交给状态）在这里手工接上。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use assemble_client::{Client, EventStream, OutgoingFile, RequestOptions, TransferOptions};
use assemble_protocol::device::NewDevice;
use assemble_protocol::events::Event;
use assemble_protocol::origin::ServerOrigin;
use assemble_protocol::primitives::{Platform, Timestamp};
use assemble_protocol::sends::{
    Direction, FailReason, SEND_PAGE_MAX, Send as SendRecord, SendQuery, SendState,
};
use assemble_server::http::TestServer;
use assemble_store::secret::SecretString;
use futures_util::StreamExt;
use sqlx::SqlitePool;
use tauri::Manager;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::registry::{
    Cancellation, Delivery, Destination, Next, TransferFailure, TransferPhase, TransferTarget,
    TransferView,
};
use crate::state::{AccountView, AppState, DeviceView, Notifier, SessionView, Settings};

/// 等一件事发生的上限。本机回环上的一切都是毫秒级的，超过它就是卡住了。
const PATIENCE: Duration = Duration::from_secs(15);

/// 装在模拟应用里的那份状态。真实的桌面端用的是窗口运行时，这里用的是测试运行时。
type Shell = AppState<tauri::test::MockRuntime>;

/// 把通知记下来，不真的弹出来：跑测试不该在屏幕上冒通知。
#[derive(Debug, Default)]
struct RecordingNotifier(Mutex<Vec<(i32, String, bool)>>);

impl Notifier for RecordingNotifier {
    fn received(&self, handle: i32, _peer: &str, file_name: &str, fell_back: bool) {
        self.0
            .lock()
            .unwrap()
            .push((handle, file_name.to_owned(), fell_back));
    }

    fn reminder(&self, _handle: i32, _title: &str, _body: &str) {}
}

/// 一台设备在服务端的身份。
struct Device {
    id: String,
    token: SecretString,
}

/// 一台"桌面端"加上一台给它发文件的设备，都连着同一个真实的服务端。
struct Fixture {
    dir: tempfile::TempDir,
    server: TestServer,
    /// 让这台设备在服务端眼里是在线的：事件流一直开着。
    online: Option<EventStream>,
    hold: CancellationToken,
    app: tauri::App<tauri::test::MockRuntime>,
    sender: Client,
    desktop_id: String,
    notifications: Arc<RecordingNotifier>,
}

impl Fixture {
    fn state(&self) -> tauri::State<'_, Shell> {
        self.app.state::<Shell>()
    }

    /// 默认的接收位置："下载/Assemble"。
    fn inbox(&self) -> PathBuf {
        default_directory(&self.dir.path().join("Downloads"))
    }

    /// 像真实运行时那样启动接收任务。
    fn start_receiving(&self) {
        let app = self.app.handle().clone();
        tauri::async_runtime::spawn(async move { run(app).await });
    }

    /// 从另一台设备发一个文件过来。
    async fn send(&self, name: &str, payload: &[u8]) -> SendRecord {
        let path = self.dir.path().join("outgoing").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, payload).unwrap();
        let cancel = CancellationToken::new();
        let file = OutgoingFile::open(&path, &cancel).await.unwrap();
        self.sender
            .send_to_device(&self.desktop_id, &file, TransferOptions::new(&cancel), None)
            .await
            .unwrap()
    }

    /// 取下一条与文件有关的事件，跳过握手与设备名单。
    async fn next_send_event(&mut self) -> SendRecord {
        let stream = self.online.as_mut().expect("the device is online");
        for _ in 0..20 {
            if let Some(Ok(Event::SendOffered { send } | Event::SendUpdated { send })) =
                stream.next().await
            {
                return send;
            }
        }
        panic!("no event about a file arrived");
    }

    /// 发送方眼里它发出去的那条记录。
    async fn as_seen_by_the_sender(&self, id: &str) -> SendRecord {
        let cancel = CancellationToken::new();
        let listed = self
            .sender
            .sends(
                &SendQuery {
                    direction: Direction::Out,
                    state: None,
                    limit: SEND_PAGE_MAX,
                },
                &RequestOptions::new(&cancel),
            )
            .await
            .unwrap();
        listed
            .into_iter()
            .find(|send| send.id == id)
            .expect("the sender still lists this send")
    }

    async fn finish(self) {
        let Self {
            dir,
            mut server,
            online,
            app,
            ..
        } = self;
        app.state::<Shell>().lifetime.cancel();
        drop(online);
        server.stop().await;
        drop(app);
        drop(dir);
    }
}

async fn register(pool: &SqlitePool, account_id: &str, name: &str) -> Device {
    let token = assemble_server::secrets::random_token().unwrap();
    let row = assemble_server::devices::register(
        pool,
        account_id,
        &NewDevice {
            name: name.into(),
            platform: Platform::Macos,
        },
        &assemble_server::secrets::token_digest(token.expose()),
    )
    .await
    .unwrap();
    Device { id: row.id, token }
}

/// 起一个真实的服务端，登记一个账户与两台设备，再把"桌面端"登录到这个账户。
async fn fixture() -> Fixture {
    fixture_with_quota(None).await
}

/// `quota` 为 `None` 时用默认的云盘容量。
async fn fixture_with_quota(quota: Option<u64>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("server");
    let server = match quota {
        None => TestServer::start_default(&data).await.unwrap(),
        Some(quota) => {
            let pool = assemble_server::db::open(&data).await.unwrap();
            let config = assemble_server::Config::development(&data);
            let config = assemble_server::Config {
                drive_quota_bytes: quota,
                ..config
            };
            TestServer::start(config, pool).await.unwrap()
        }
    };
    let pool = server.state.pool.clone();
    let account = assemble_server::accounts::create(
        &pool,
        "alex",
        "Alex",
        SecretString::new("correct horse battery staple"),
    )
    .await
    .unwrap();
    let desktop = register(&pool, &account.id, "Mac").await;
    let other = register(&pool, &account.id, "工作电脑").await;

    let desktop_client = Client::new(server.origin.clone()).unwrap();
    desktop_client.set_bearer(Some(desktop.token.clone()));
    let sender = Client::new(server.origin.clone()).unwrap();
    sender.set_bearer(Some(other.token));

    let app = tauri::test::mock_app();
    let notifications = Arc::new(RecordingNotifier::default());
    app.manage(AppState::new(
        app.handle().clone(),
        desktop_client,
        None,
        Settings::default(),
        dir.path().join("Downloads"),
        notifications.clone(),
    ));
    let hold = CancellationToken::new();
    let online = Some(
        app.state::<Shell>()
            .client
            .clone()
            .events(hold.child_token()),
    );
    {
        let state = app.state::<Shell>();
        state.set_session(SessionView::SignedIn {
            account: AccountView {
                id: account.id.clone(),
                username: account.username.clone(),
                display_name: account.display_name.clone(),
            },
            device: DeviceView {
                id: desktop.id.clone(),
                name: "Mac".into(),
                platform: "macos".into(),
                online: true,
            },
        });
        state.begin_session();
    }
    let mut fixture = Fixture {
        dir,
        server,
        online,
        hold,
        app,
        sender,
        desktop_id: desktop.id,
        notifications,
    };

    // 事件流是异步连上的。发送方只能发给在线的设备，所以先等它真的在线。
    let deadline = Instant::now() + PATIENCE;
    while !is_online(&mut fixture).await {
        assert!(Instant::now() < deadline, "the desktop never came online");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    fixture
}

async fn is_online(fixture: &mut Fixture) -> bool {
    fixture
        .sender
        .devices(&RequestOptions::new(&fixture.hold))
        .await
        .unwrap()
        .iter()
        .any(|device| device.id == fixture.desktop_id && device.online)
}

/// 本机已经有了结局，而且服务端已经知道：文件收好了、或者收不下来这件事已经报上去了。
fn acknowledged(state: &Shell, id: &str) -> bool {
    let finished = state.snapshot().transfers.iter().any(|transfer| {
        transfer.id == id
            && matches!(
                transfer.phase,
                TransferPhase::Complete | TransferPhase::Failed
            )
    });
    finished && state.settlement_of(id).is_none()
}

/// 等到 `condition` 成立；太久不成立就失败，并说出在等什么。
async fn until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 云盘名单里已经有这个名字的文件。
///
/// 名单是传完之后才去拿的，比传输记录晚一步。传完就去看名单，机器一忙就来不及：
/// 2026-11-25 在负载下跑整套测试时，两条云盘的测试各这样失败过一次。
fn listed(fixture: &Fixture, name: &str) -> bool {
    fixture
        .state()
        .snapshot()
        .drive
        .is_some_and(|listing| listing.items.iter().any(|item| item.name == name))
}

/// 等到接收目录里正好是这几个文件。
///
/// 发布是"先链接、再删暂存名"：文件一落地的那一瞬间，暂存名可能还在。
/// 同一天负载下，收文件的那条测试就这样失败过一次。
async fn until_inbox_holds(fixture: &Fixture, names: &[&str]) {
    until("the inbox to hold exactly the published files", || {
        entries(&fixture.inbox()) == names
    })
    .await;
}

fn entries(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn offered(id: &str, name: &str) -> SendRecord {
    SendRecord {
        id: id.into(),
        direction: Direction::In,
        peer_device_id: "other".into(),
        peer_device_name: "工作电脑".into(),
        name: name.into(),
        size: 1_000,
        mime_type: None,
        sha256: "a".repeat(64),
        state: SendState::Offered,
        failure: None,
        created_at: Timestamp::now(),
        updated_at: Timestamp::now(),
    }
}

fn with_state(id: &str, state: SendState) -> SendRecord {
    let mut send = offered(id, "a.txt");
    send.state = state;
    send
}

#[tokio::test]
async fn a_file_from_another_device_is_saved_verified_and_accepted() {
    let mut f = fixture().await;
    f.start_receiving();
    let payload: Vec<u8> = (0..300_000u32).map(|index| (index % 251) as u8).collect();

    let sent = f.send("合同.pdf", &payload).await;
    let offered = f.next_send_event().await;
    assert_eq!(offered.id, sent.id);
    f.state().offer(&offered);

    let saved = f.inbox().join("合同.pdf");
    until("the file to be saved", || saved.exists()).await;
    assert_eq!(std::fs::read(&saved).unwrap(), payload);
    // 目录里最后只有它自己：暂存文件没有留下来。发布是"先链接、再删暂存名"，
    // 文件一出现的那一瞬间暂存名还在，所以这里要等，不能看见了就数。
    until_inbox_holds(&f, &["合同.pdf"]).await;

    // 回执送到了：发送方那边看到"对方已收下"，本机也不再把它当作待补的回执。
    until("the receipt to be acknowledged", || {
        acknowledged(&f.state(), &sent.id)
    })
    .await;
    let seen = f.as_seen_by_the_sender(&sent.id).await;
    assert_eq!(seen.state, SendState::Delivered);
    assert_eq!(seen.failure, None);

    let snapshot = f.state().snapshot();
    assert_eq!(snapshot.transfers.len(), 1);
    assert_eq!(snapshot.transfers[0].phase, TransferPhase::Complete);
    assert!((snapshot.transfers[0].progress - 1.0).abs() < 1e-9);
    assert_eq!(snapshot.transfers[0].saved_as, None);
    assert_eq!(snapshot.transfers[0].failure, None);
    assert_eq!(snapshot.receive.custom_folder, None);

    // 通知已经登记，点它找得回文件；没退回默认位置，所以不多说什么。
    let notifications = f.notifications.0.lock().unwrap().clone();
    assert_eq!(
        notifications,
        [(1, "合同.pdf".to_owned(), false)],
        "the first notification gets handle 1"
    );
    assert_eq!(
        f.state().saved_file_for_handle(1),
        Some(std::fs::canonicalize(&saved).unwrap())
    );
    // 通知的标识互不相同。
    assert_eq!(f.state().fresh_handle(), 2);
    f.finish().await;
}

#[tokio::test]
async fn a_file_that_arrived_unnoticed_is_found_by_the_next_check() {
    let f = fixture().await;
    // 事件流上的事件没人看（相当于断线重连之后）：文件只能靠对账找回来。
    let sent = f.send("会议录音.m4a", &b"audio bytes".repeat(4_000)).await;

    let cancel = CancellationToken::new();
    resync(&f.state(), &cancel).await.unwrap();
    // 重复对账不会把同一个文件排两遍。
    resync(&f.state(), &cancel).await.unwrap();
    let snapshot = f.state().snapshot();
    assert_eq!(snapshot.transfers.len(), 1);
    assert_eq!(snapshot.transfers[0].phase, TransferPhase::Queued);

    f.start_receiving();
    let saved = f.inbox().join("会议录音.m4a");
    until("the file to be saved", || saved.exists()).await;
    until("the receipt to be acknowledged", || {
        acknowledged(&f.state(), &sent.id)
    })
    .await;
    assert_eq!(
        f.as_seen_by_the_sender(&sent.id).await.state,
        SendState::Delivered
    );
    f.finish().await;
}

#[tokio::test]
async fn a_file_with_a_taken_name_is_saved_beside_the_earlier_one() {
    let mut f = fixture().await;
    f.start_receiving();

    f.send("报告.txt", b"first version").await;
    let first = f.next_send_event().await;
    f.state().offer(&first);
    until("the receipt to be acknowledged", || {
        acknowledged(&f.state(), &first.id)
    })
    .await;

    f.send("报告.txt", b"second version, different").await;
    let second = f.next_send_event().await;
    f.state().offer(&second);
    let numbered = f.inbox().join("报告 (2).txt");
    until("the second copy to be saved", || numbered.exists()).await;
    until("the receipt to be acknowledged", || {
        acknowledged(&f.state(), &second.id)
    })
    .await;

    // 先来的那份一个字节都没动。
    assert_eq!(
        std::fs::read(f.inbox().join("报告.txt")).unwrap(),
        b"first version"
    );
    assert_eq!(
        std::fs::read(&numbered).unwrap(),
        b"second version, different"
    );
    assert_eq!(entries(&f.inbox()), ["报告 (2).txt", "报告.txt"]);

    // 界面如实说出了它实际存成了什么名字；两条记录各自指向自己的文件。
    let snapshot = f.state().snapshot();
    assert_eq!(snapshot.transfers[0].id, second.id);
    assert_eq!(snapshot.transfers[0].name, "报告.txt");
    assert_eq!(
        snapshot.transfers[0].saved_as.as_deref(),
        Some("报告 (2).txt")
    );
    assert_eq!(
        f.state().saved_file(&first.id),
        Some(std::fs::canonicalize(f.inbox().join("报告.txt")).unwrap())
    );
    assert_eq!(
        f.state().saved_file(&second.id),
        Some(std::fs::canonicalize(&numbered).unwrap())
    );
    f.finish().await;
}

#[tokio::test]
async fn files_are_received_one_at_a_time_in_the_order_they_arrived() {
    let mut f = fixture().await;
    // 接收还没开始就先来三个同名文件。
    let first = f.send("照片.jpg", b"one").await;
    let second = f.send("照片.jpg", b"two").await;
    let third = f.send("照片.jpg", b"three").await;
    for _ in 0..3 {
        let send = f.next_send_event().await;
        f.state().offer(&send);
    }
    assert_eq!(f.state().snapshot().transfers.len(), 3);

    f.start_receiving();
    until("all three to be acknowledged", || {
        let state = f.state();
        [&first.id, &second.id, &third.id]
            .iter()
            .all(|id| acknowledged(&state, id))
    })
    .await;
    // 名字按先来后到分：谁先到谁用原名。
    assert_eq!(
        entries(&f.inbox()),
        ["照片 (2).jpg", "照片 (3).jpg", "照片.jpg"]
    );
    assert_eq!(std::fs::read(f.inbox().join("照片.jpg")).unwrap(), b"one");
    assert_eq!(
        std::fs::read(f.inbox().join("照片 (2).jpg")).unwrap(),
        b"two"
    );
    assert_eq!(
        std::fs::read(f.inbox().join("照片 (3).jpg")).unwrap(),
        b"three"
    );
    f.finish().await;
}

#[tokio::test]
async fn a_file_that_cannot_be_saved_is_reported_to_the_sender_with_the_reason() {
    let mut f = fixture().await;
    // "下载/Assemble"这个位置上现在是一个文件：默认的接收目录建不出来。
    std::fs::create_dir_all(f.dir.path().join("Downloads")).unwrap();
    std::fs::write(f.inbox(), b"not a directory").unwrap();

    let sent = f.send("合同.pdf", b"content").await;
    let offered = f.next_send_event().await;
    f.state().offer(&offered);
    f.start_receiving();

    until("the receipt to be acknowledged", || {
        acknowledged(&f.state(), &sent.id)
    })
    .await;
    let seen = f.as_seen_by_the_sender(&sent.id).await;
    assert_eq!(seen.state, SendState::Failed);
    assert_eq!(seen.failure, Some(FailReason::WriteFailed));

    let snapshot = f.state().snapshot();
    assert_eq!(snapshot.transfers[0].phase, TransferPhase::Failed);
    assert_eq!(
        snapshot.transfers[0].failure,
        Some(TransferFailure::WriteFailed)
    );
    // 没有文件可显示，也就没有通知。
    assert!(f.notifications.0.lock().unwrap().is_empty());
    assert_eq!(f.state().saved_file(&sent.id), None);
    f.finish().await;
}

#[tokio::test]
async fn a_withdrawn_file_is_never_received_and_leaves_the_server_state_alone() {
    let mut f = fixture().await;
    let sent = f.send("合同.pdf", b"sent by mistake").await;
    let offered = f.next_send_event().await;
    f.state().offer(&offered);
    assert_eq!(
        f.state().snapshot().transfers[0].phase,
        TransferPhase::Queued
    );

    // 接收还没开始，对面就撤回了。
    let cancel = CancellationToken::new();
    f.sender
        .send_cancel(&sent.id, &RequestOptions::new(&cancel))
        .await
        .unwrap();
    let update = f.next_send_event().await;
    assert_eq!(update.state, SendState::Cancelled);
    f.state().updated(&update);

    f.start_receiving();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!f.inbox().exists());
    assert_eq!(f.state().next_job(), Next::Nothing);
    let snapshot = f.state().snapshot();
    assert_eq!(snapshot.transfers[0].phase, TransferPhase::Cancelled);
    assert_eq!(snapshot.transfers[0].failure, None);
    // 服务端那边没有被改动：还是发送方留下的"已撤回"。
    assert_eq!(
        f.as_seen_by_the_sender(&sent.id).await.state,
        SendState::Cancelled
    );
    assert!(f.notifications.0.lock().unwrap().is_empty());
    f.finish().await;
}

#[tokio::test]
async fn withdrawing_stops_the_receive_that_is_under_way() {
    let f = fixture().await;
    {
        let state = f.state();
        state.offer(&offered("s1", "a.txt"));
        let taken = state.begin_receive("s1");
        assert!(
            matches!(taken, Some(Delivery::FromDevice(send)) if send.id == "s1"),
            "the queued file is picked up"
        );

        let under_way = CancellationToken::new();
        let tracked = state.track_transfer("s1", under_way.clone());
        assert!(!under_way.is_cancelled());
        // 服务端只是又描述了一次这个文件：不算撤回。
        state.updated(&with_state("s1", SendState::Offered));
        assert!(!under_way.is_cancelled());
        // 对面撤回：那一次接收到此为止。
        state.updated(&with_state("s1", SendState::Cancelled));
        assert!(under_way.is_cancelled());
        assert_eq!(
            state.snapshot().transfers[0].phase,
            TransferPhase::Cancelled
        );
        drop(tracked);

        // 已经不在接收了：再停一次什么都不做；别的接收不受影响。
        let other = CancellationToken::new();
        let _tracked = state.track_transfer("s2", other.clone());
        state.stop_transfer("s1");
        assert!(!other.is_cancelled());
    }
    f.finish().await;
}

#[tokio::test]
async fn an_unusable_receive_folder_falls_back_to_the_default_and_says_so() {
    let mut f = fixture().await;
    let gone = f.dir.path().join("移动硬盘").join("收件");
    f.state().set_receive_folder(Some(&gone));
    f.start_receiving();

    let sent = f.send("合同.pdf", b"content").await;
    let offered = f.next_send_event().await;
    f.state().offer(&offered);
    let saved = f.inbox().join("合同.pdf");
    until("the file to be saved", || saved.exists()).await;
    until("the receipt to be acknowledged", || {
        acknowledged(&f.state(), &sent.id)
    })
    .await;

    // 设置没有被悄悄改掉：下次那个盘插上了，文件还会存到那里。
    assert_eq!(
        f.state().settings().receive_directory.as_deref(),
        Some(gone.as_path())
    );
    assert_eq!(
        f.state().snapshot().receive.custom_folder.as_deref(),
        Some("收件")
    );
    // 但用户被告知了这一次存到了别处：通知与应用里的提示都说了。
    assert_eq!(
        *f.notifications.0.lock().unwrap(),
        [(1, "合同.pdf".to_owned(), true)]
    );
    let notice = f
        .state()
        .snapshot()
        .notice
        .expect("the fallback is announced");
    assert!(
        notice.message.contains("原来设置的接收文件夹不可用"),
        "{}",
        notice.message
    );
    f.finish().await;
}

#[tokio::test]
async fn staging_files_left_by_an_interrupted_run_are_swept_before_receiving_starts() {
    let f = fixture().await;
    let inbox = f.inbox();
    std::fs::create_dir_all(&inbox).unwrap();
    let leftover = inbox.join("合同.pdf.0123456789abcdef0123456789abcdef.part");
    std::fs::write(&leftover, b"half a file").unwrap();
    std::fs::write(inbox.join("笔记.txt"), b"mine").unwrap();
    // 用户设置的文件夹里也清一遍。
    let chosen = f.dir.path().join("chosen");
    std::fs::create_dir_all(&chosen).unwrap();
    let there = chosen.join("a.bin.00000000000000000000000000000000.part");
    std::fs::write(&there, b"").unwrap();
    f.state().set_receive_folder(Some(&chosen));

    f.start_receiving();
    // 两个文件夹是先后清的：先默认位置，再用户设置的那个。等前一个清完就去看后一个，
    // 机器一忙就赶在它前面——2026-11-25 持续集成的 macOS 那一栏就这样红过一次。
    until("the sweep to run in both folders", || {
        !leftover.exists() && !there.exists()
    })
    .await;
    assert_eq!(std::fs::read(inbox.join("笔记.txt")).unwrap(), b"mine");
    f.finish().await;
}

// ---- 取内容时的各种结局 ----
//
// 下面不连真实的服务端，而是一台只会按剧本应答的"服务端"：
// 要制造"连接中途断开""服务端出错"这些情况，只能由测试自己控制对端。

/// 成功时应答的正文：一千个字节，内容固定，测试据此算摘要。
fn body() -> Vec<u8> {
    (0..1_000u32).map(|index| (index % 251) as u8).collect()
}

/// 一个只会按剧本应答的服务端。剧本的每一项是（状态码，正文是否完整）；
/// 正文不完整时只送十个字节就把连接断开。
struct Scripted {
    origin: ServerOrigin,
    _task: tokio::task::JoinHandle<()>,
}

async fn scripted(responses: Vec<(u16, bool)>) -> Scripted {
    async fn serve(
        listener: tokio::net::TcpListener,
        responses: Mutex<std::collections::VecDeque<(u16, bool)>>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut socket, _)) = listener.accept().await {
            let Some((status, full)) = responses.lock().unwrap().pop_front() else {
                return;
            };
            // 把请求头读完再应答；这台服务端不关心请求的内容。
            let mut request = [0u8; 4_096];
            let mut read = 0;
            loop {
                let got = socket.read(&mut request[read..]).await.unwrap_or(0);
                if got == 0 {
                    break;
                }
                read += got;
                if request[..read].ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let reason = match status {
                200 => "OK",
                401 => "Unauthorized",
                403 => "Forbidden",
                404 => "Not Found",
                409 => "Conflict",
                _ => "Server Error",
            };
            let head = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/octet-stream\r\ncontent-length: 1000\r\n\r\n"
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            if status == 200 {
                let bytes = body();
                if full {
                    socket.write_all(&bytes).await.unwrap();
                } else {
                    // 不说一声就断：客户端看到的是"说好一千字节，只来了十个"。
                    socket.write_all(&bytes[..10]).await.unwrap();
                }
            } else {
                socket
                    .write_all(b"{\"error\":{\"code\":\"X\",\"message\":\"x\"}}")
                    .await
                    .unwrap();
            }
            let _ = socket.shutdown().await;
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(serve(listener, Mutex::new(responses.into_iter().collect())));
    Scripted {
        origin: ServerOrigin::parse(&format!("http://{address}")).unwrap(),
        _task: task,
    }
}

/// 一台"桌面端"，连的是给定地址的服务端，身边没有别的设备。
fn desktop_pointed_at(
    origin: &ServerOrigin,
    downloads: &Path,
) -> tauri::App<tauri::test::MockRuntime> {
    let client = Client::new(origin.clone()).unwrap();
    let app = tauri::test::mock_app();
    app.manage(AppState::new(
        app.handle().clone(),
        client,
        None,
        Settings::default(),
        downloads.to_path_buf(),
        Arc::new(RecordingNotifier::default()),
    ));
    let state = app.state::<Shell>();
    state.set_session(SessionView::SignedIn {
        account: AccountView {
            id: "a1".into(),
            username: "alex".into(),
            display_name: "Alex".into(),
        },
        device: DeviceView {
            id: "d1".into(),
            name: "Mac".into(),
            platform: "macos".into(),
            online: true,
        },
    });
    state.begin_session();
    app
}

/// 结局里去掉本机路径之后的样子：路径每次都不一样，没法直接比。
fn in_a_nutshell(outcome: &Outcome) -> (&'static str, Option<ReceiveFailure>) {
    match outcome {
        Outcome::Saved { .. } => ("saved", None),
        Outcome::Retry => ("retry", None),
        Outcome::Failed(failure) => ("failed", Some(*failure)),
        Outcome::Interrupted => ("interrupted", None),
        Outcome::Unauthorized => ("unauthorized", None),
        Outcome::Gone => ("gone", None),
        Outcome::Changed => ("changed", None),
    }
}

#[tokio::test]
async fn failures_to_fetch_content_are_sorted_by_who_should_hear_about_them() {
    // （状态码，正文是否完整，声明的摘要是否与正文相符）
    let cases: [(u16, bool, bool); 8] = [
        (200, true, true),
        (200, true, false),
        (200, false, true),
        (500, false, true),
        (404, false, true),
        (409, false, true),
        (401, false, true),
        (403, false, true),
    ];
    let expected: [(bool, &'static str, Option<ReceiveFailure>); 8] = [
        (true, "saved", None),
        (false, "failed", Some(ReceiveFailure::VerificationFailed)),
        // 连接断了、服务端出错：说不准是谁的问题，过一会儿再试，不怪任何一方的磁盘。
        (false, "retry", None),
        (false, "retry", None),
        (false, "gone", None),
        (false, "changed", None),
        (false, "unauthorized", None),
        (false, "changed", None),
    ];
    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("Downloads");

    for (index, (status, full, honest)) in cases.into_iter().enumerate() {
        let server = scripted(vec![(status, full)]).await;
        let app = desktop_pointed_at(&server.origin, &downloads);
        let state = app.state::<Shell>();
        let mut send = offered(&format!("s{index}"), "a.bin");
        send.size = 1_000;
        send.sha256 = assemble_transfer::hash::digest(&if honest {
            body()
        } else {
            b"something else entirely".to_vec()
        });
        state.offer(&send);
        assert!(
            state.begin_receive(&send.id).is_some(),
            "the send is queued"
        );

        let cancel = CancellationToken::new();
        let outcome = pull(&state, &claim(&Delivery::FromDevice(send.clone())), &cancel).await;
        let saved = match &outcome {
            Outcome::Saved { received, .. } => Some(received.path.clone()),
            _ => None,
        };
        let (on_disk, kind, failure) = expected[index];
        assert_eq!(saved.is_some(), on_disk, "status {status}");
        assert_eq!(in_a_nutshell(&outcome), (kind, failure), "status {status}");

        // 只有核对通过的那一种在磁盘上留下了文件，且内容一字不差。
        let inbox = default_directory(&downloads);
        if let Some(path) = &saved {
            assert_eq!(std::fs::read(path).unwrap(), body());
            let folder = std::fs::canonicalize(&inbox).unwrap();
            assert_eq!(path.parent(), Some(folder.as_path()));
            assert_eq!(entries(&inbox), ["a.bin"]);
            std::fs::remove_dir_all(&inbox).unwrap();
        } else {
            assert_eq!(entries(&inbox), Vec::<String>::new());
        }
        app.state::<Shell>().lifetime.cancel();
        drop(app);
    }
    drop(dir);
}

#[tokio::test]
async fn the_local_record_and_the_server_record_agree_at_every_step() {
    let mut f = fixture().await;
    f.start_receiving();

    let sent = f.send("视频.mp4", &vec![9u8; 150_000]).await;
    let offered = f.next_send_event().await;
    f.state().offer(&offered);

    // 走到头之后：本机说"已存好"，服务端说"对方已收下"，两边说的是同一个文件。
    until("the receipt to be acknowledged", || {
        acknowledged(&f.state(), &sent.id)
    })
    .await;
    let mut snapshot = f.state().snapshot();
    let local = snapshot.transfers.remove(0);
    let remote = f.as_seen_by_the_sender(&sent.id).await;
    assert_eq!(local.phase, TransferPhase::Complete);
    assert_eq!(remote.state, SendState::Delivered);
    assert_eq!(local.name, remote.name);
    assert_eq!(local.size, remote.size);
    assert_eq!(local.peer, "工作电脑");
    assert_eq!(f.state().next_job(), Next::Nothing);

    // 收完之后服务端不再保管这份内容：再取一次会被拒绝。
    let cancel = CancellationToken::new();
    let error = f
        .state()
        .client
        .send_content(&sent.id, TransferOptions::new(&cancel), None)
        .await
        .expect_err("the content is gone once it is delivered");
    assert_eq!(error.status(), Some(409));
    f.finish().await;
}

// ---- 发出的 ----
//
// 这台设备发、对面那台收。对面由测试自己扮演：收下，或者干脆不在线。

fn payload(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).expect("below 251"))
        .collect()
}

/// 一个可以发出去的文件：写进磁盘、算好摘要。
async fn pick(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> OutgoingFile {
    let path = dir.path().join("outgoing").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    OutgoingFile::open(&path, &CancellationToken::new())
        .await
        .unwrap()
}

/// "工作电脑"这台设备在界面上看的样子。
async fn work_pc(f: &Fixture) -> DeviceView {
    f.sender
        .devices(&RequestOptions::new(&f.hold))
        .await
        .unwrap()
        .into_iter()
        .find(|device| device.name == "工作电脑")
        .map(|device| DeviceView::from(&device))
        .expect("the other device is registered")
}

/// 让"工作电脑"上线：把它的 event 流开着。返回这个流，调用方要一直持有它。
fn bring_online(f: &Fixture) -> EventStream {
    f.sender.clone().events(f.hold.child_token())
}

/// 等到 `id` 这台设备在服务端眼里是在线的。
async fn wait_until_online(f: &Fixture, id: &str) {
    let deadline = Instant::now() + PATIENCE;
    while !is_online_for(f, id).await {
        assert!(
            Instant::now() < deadline,
            "the other device never came online"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn is_online_for(f: &Fixture, id: &str) -> bool {
    f.sender
        .devices(&RequestOptions::new(&f.hold))
        .await
        .unwrap()
        .iter()
        .any(|device| device.id == id && device.online)
}

/// 对面的事件流上，下一条与文件有关的事件。
async fn next_send_on(stream: &mut EventStream) -> SendRecord {
    for _ in 0..20 {
        if let Some(Ok(Event::SendOffered { send } | Event::SendUpdated { send })) =
            stream.next().await
        {
            return send;
        }
    }
    panic!("no event about a file reached the other device");
}

fn view_of(f: &Fixture, id: &str) -> TransferView {
    f.state()
        .snapshot()
        .transfers
        .into_iter()
        .find(|view| view.id == id)
        .unwrap_or_else(|| panic!("{id} is not listed"))
}

/// 对面（收件方）眼里收到的记录。
async fn as_seen_by_the_receiver(f: &Fixture, id: &str) -> SendRecord {
    let cancel = CancellationToken::new();
    let listed = f
        .sender
        .sends(
            &SendQuery {
                direction: Direction::In,
                state: None,
                limit: SEND_PAGE_MAX,
            },
            &RequestOptions::new(&cancel),
        )
        .await
        .unwrap();
    listed
        .into_iter()
        .find(|send| send.id == id)
        .expect("the receiver still lists this send")
}

async fn received_by_the_receiver(f: &Fixture, id: &str) -> Vec<u8> {
    use tokio::io::AsyncReadExt;

    let cancel = CancellationToken::new();
    let download = f
        .sender
        .send_content(id, TransferOptions::new(&cancel), None)
        .await
        .unwrap();
    let mut bytes = Vec::new();
    download
        .into_reader()
        .read_to_end(&mut bytes)
        .await
        .unwrap();
    bytes
}

#[tokio::test]
async fn a_file_picked_here_reaches_the_other_device_and_is_complete_once_it_is_accepted() {
    let mut f = fixture().await;
    // 对面那台设备连上了事件流，所以是在线的。
    let mut receiver = bring_online(&f);
    let target = work_pc(&f).await;
    wait_until_online(&f, &target.id).await;
    f.start_receiving();

    let bytes = payload(200_000);
    let file = pick(&f.dir, "季度报表.xlsx", &bytes).await;
    f.state().set_draft(vec![file.clone()]);
    // 面板看得到名字与大小，看不到位置。
    let draft = f.state().snapshot().draft.expect("the draft is shown");
    assert_eq!(draft.files.len(), 1);
    assert_eq!(draft.files[0].name, "季度报表.xlsx");
    assert_eq!(draft.files[0].size, bytes.len() as u64);
    let shown = serde_json::to_string(&draft).unwrap();
    assert!(!shown.contains("outgoing"), "{shown}");

    // 用户在面板里点了那台设备。
    let ids = f.state().queue_draft(Destination::Device(target.clone()));
    assert_eq!(ids.len(), 1);
    assert!(f.state().snapshot().draft.is_none(), "the draft is used up");

    // 对面收到"有人给你发了文件"，内容一字不差。
    let offered = next_send_on(&mut receiver).await;
    assert_eq!(offered.peer_device_name, "Mac");
    assert_eq!(received_by_the_receiver(&f, &offered.id).await, bytes);
    // 这台设备这边：文件已经发出去了，在等对方接收。
    until("the file to be uploaded", || {
        view_of(&f, &ids[0]).phase == TransferPhase::Waiting
    })
    .await;

    // 对面收下了。这台设备的事件流上出现"对方已收下"，事件循环把它交给状态。
    let cancel = CancellationToken::new();
    f.sender
        .send_accept(&offered.id, &RequestOptions::new(&cancel))
        .await
        .unwrap();
    let update = f.next_send_event().await;
    assert_eq!(update.state, SendState::Delivered);
    assert!(!f.state().updated(&update), "nothing to withdraw");

    let view = view_of(&f, &ids[0]);
    assert_eq!(view.direction, Direction::Out);
    assert_eq!(view.peer, "工作电脑");
    assert_eq!(view.name, "季度报表.xlsx");
    assert_eq!(view.size, bytes.len() as u64);
    assert_eq!(view.phase, TransferPhase::Complete);
    assert!(!view.retryable);
    assert_eq!(view.saved_as, None);
    f.finish().await;
}

#[tokio::test]
async fn a_device_that_is_offline_by_the_time_the_upload_lands_means_the_file_was_never_sent() {
    let f = fixture().await;
    // "工作电脑"没有连着事件流：它是离线的。
    let target = work_pc(&f).await;
    f.start_receiving();

    let file = pick(&f.dir, "合同.pdf", &payload(50_000)).await;
    f.state().set_draft(vec![file.clone()]);
    let ids = f.state().queue_draft(Destination::Device(target.clone()));

    until("the refusal to be recorded", || {
        view_of(&f, &ids[0]).phase == TransferPhase::Failed
    })
    .await;
    let view = view_of(&f, &ids[0]);
    assert_eq!(view.failure, Some(TransferFailure::DeviceOffline));
    // 文件没到服务端：可以再发一次。
    assert!(view.retryable);
    // 服务端什么都没留下，对面什么都收不到。
    let cancel = CancellationToken::new();
    let listed = f
        .sender
        .sends(
            &SendQuery {
                direction: Direction::In,
                state: None,
                limit: SEND_PAGE_MAX,
            },
            &RequestOptions::new(&cancel),
        )
        .await
        .unwrap();
    assert_eq!(listed, Vec::new());
    f.finish().await;
}

#[tokio::test]
async fn cancelling_a_sent_file_withdraws_it_before_the_other_device_looks() {
    let f = fixture().await;
    let mut receiver = bring_online(&f);
    let target = work_pc(&f).await;
    wait_until_online(&f, &target.id).await;
    f.start_receiving();

    let file = pick(&f.dir, "合同.pdf", &payload(10_000)).await;
    f.state().set_draft(vec![file.clone()]);
    let ids = f.state().queue_draft(Destination::Device(target.clone()));
    let offered = next_send_on(&mut receiver).await;
    until("the file to be uploaded", || {
        view_of(&f, &ids[0]).phase == TransferPhase::Waiting
    })
    .await;

    // 用户在传输列表里点了"取消"：文件已经在服务端，得去撤回。
    assert_eq!(
        cancel(&f.state(), &ids[0]).await,
        Cancellation::OnServer(offered.id.clone())
    );
    assert_eq!(view_of(&f, &ids[0]).phase, TransferPhase::Cancelled);
    // 对面收到"已撤回"，内容也取不到了。
    let update = next_send_on(&mut receiver).await;
    assert_eq!(update.state, SendState::Cancelled);
    let cancel_token = CancellationToken::new();
    let error = f
        .sender
        .send_content(&offered.id, TransferOptions::new(&cancel_token), None)
        .await
        .expect_err("a withdrawn send has no content");
    assert_eq!(error.status(), Some(409));
    // 到头了：再取消什么都不做。
    assert_eq!(cancel(&f.state(), &ids[0]).await, Cancellation::Nothing);
    f.finish().await;
}

#[tokio::test]
async fn an_upload_abandoned_halfway_is_withdrawn_once_it_turns_out_to_have_arrived() {
    let f = fixture().await;
    let _receiver = bring_online(&f);
    let target = work_pc(&f).await;
    wait_until_online(&f, &target.id).await;

    let file = pick(&f.dir, "合同.pdf", &payload(10_000)).await;
    f.state().set_draft(vec![file.clone()]);
    let ids = f.state().queue_draft(Destination::Device(target.clone()));
    // 接收任务没在跑：模拟"正在上传"。
    assert!(f.state().begin_send(&ids[0]).is_some());
    assert_eq!(f.state().cancel_transfer(&ids[0]), Cancellation::Local);
    assert_eq!(view_of(&f, &ids[0]).phase, TransferPhase::Cancelled);

    // 上传其实已经到了服务端，只是回执回来晚了一步。
    let cancel = CancellationToken::new();
    let sent = f
        .state()
        .client
        .send_to_device(&target.id, &file, TransferOptions::new(&cancel), None)
        .await
        .unwrap();
    assert!(
        f.state().send_uploaded(&ids[0], &sent),
        "the file must be withdrawn on the server"
    );
    // 记录没有改回"等对方接收"。
    assert_eq!(view_of(&f, &ids[0]).phase, TransferPhase::Cancelled);
    withdraw(&f.state(), &sent.id, &cancel).await;
    assert_eq!(
        as_seen_by_the_receiver(&f, &sent.id).await.state,
        SendState::Cancelled
    );
    f.finish().await;
}

#[tokio::test]
async fn an_abandoned_upload_that_arrived_unnoticed_is_withdrawn_at_the_next_check() {
    let f = fixture().await;
    let mut receiver = bring_online(&f);
    let target = work_pc(&f).await;
    wait_until_online(&f, &target.id).await;

    let file = pick(&f.dir, "合同.pdf", &payload(10_000)).await;
    f.state().set_draft(vec![file.clone()]);
    let ids = f.state().queue_draft(Destination::Device(target.clone()));
    assert!(f.state().begin_send(&ids[0]).is_some());
    assert_eq!(f.state().cancel_transfer(&ids[0]), Cancellation::Local);

    // 这一回回执彻底丢了：本机不知道服务端给了它哪个标识。
    let cancel = CancellationToken::new();
    let sent = f
        .state()
        .client
        .send_to_device(&target.id, &file, TransferOptions::new(&cancel), None)
        .await
        .unwrap();
    assert!(f.state().abandoned(&sent));

    // 对账的时候认出来并撤回。
    resync(&f.state(), &cancel).await.unwrap();
    assert_eq!(
        as_seen_by_the_receiver(&f, &sent.id).await.state,
        SendState::Cancelled
    );
    // 对面的事件流上先到的是当初那条"有人给你发了文件"，然后才是"已撤回"。
    let first = next_send_on(&mut receiver).await;
    assert_eq!(first.state, SendState::Offered);
    let update = next_send_on(&mut receiver).await;
    assert_eq!(update.state, SendState::Cancelled);
    assert_eq!(view_of(&f, &ids[0]).phase, TransferPhase::Cancelled);
    f.finish().await;
}

#[tokio::test]
async fn a_file_that_vanishes_before_it_is_sent_can_be_sent_again_once_it_is_back() {
    let f = fixture().await;
    let _receiver = bring_online(&f);
    let target = work_pc(&f).await;
    wait_until_online(&f, &target.id).await;

    let bytes = payload(10_000);
    let path = f.dir.path().join("outgoing").join("临时.bin");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let file = OutgoingFile::open(&path, &CancellationToken::new())
        .await
        .unwrap();
    f.state().set_draft(vec![file.clone()]);
    let ids = f.state().queue_draft(Destination::Device(target.clone()));
    // 还没开始发，文件就被移走了。
    std::fs::remove_file(&path).unwrap();
    f.start_receiving();

    until("the failure to be recorded", || {
        view_of(&f, &ids[0]).phase == TransferPhase::Failed
    })
    .await;
    let view = view_of(&f, &ids[0]);
    assert_eq!(view.failure, Some(TransferFailure::FileUnreadable));
    assert!(view.retryable);
    // 失败之后不会自己重试：得用户点"重发"。
    assert_eq!(f.state().next_job(), Next::Nothing);

    // 文件回来了：再发一次，这回发到了服务端。
    std::fs::write(&path, &bytes).unwrap();
    assert!(retry_send(&f.state(), &ids[0]));
    until("the file to be uploaded", || {
        view_of(&f, &ids[0]).phase == TransferPhase::Waiting
    })
    .await;
    // 已经在服务端了：不能再发一次。
    assert!(!retry_send(&f.state(), &ids[0]));
    f.finish().await;
}

#[tokio::test]
async fn cancelling_a_receive_tells_the_sender_why_the_file_stopped() {
    let mut f = fixture().await;
    let sent = f.send("合同.pdf", b"a file the user does not want").await;
    let offered = f.next_send_event().await;
    f.state().offer(&offered);

    // 排着队还没开始收，用户点了"取消"。
    assert_eq!(cancel(&f.state(), &sent.id).await, Cancellation::Local);
    let view = view_of(&f, &sent.id);
    assert_eq!(view.phase, TransferPhase::Failed);
    assert_eq!(view.failure, Some(TransferFailure::Cancelled));

    // 原因报告给了发送方；磁盘上什么都没有。
    f.start_receiving();
    until("the cancellation to be reported", || {
        acknowledged(&f.state(), &sent.id)
    })
    .await;
    let seen = f.as_seen_by_the_sender(&sent.id).await;
    assert_eq!(seen.state, SendState::Failed);
    assert_eq!(seen.failure, Some(FailReason::Cancelled));
    assert!(!f.inbox().exists());
    f.finish().await;
}

// ---- 云盘 ----
//
// 存进云盘、从云盘取回，走的也是后台任务那一个队列。服务端是真实的。

/// 从云盘里取回一个文件的全部内容（由另一台设备去取，用来核对）。
async fn on_the_drive(f: &Fixture, id: &str) -> Vec<u8> {
    use tokio::io::AsyncReadExt;

    let download = f
        .sender
        .drive_content(id, TransferOptions::new(&f.hold), None)
        .await
        .unwrap();
    let mut bytes = Vec::new();
    download
        .into_reader()
        .read_to_end(&mut bytes)
        .await
        .unwrap();
    bytes
}

#[tokio::test]
async fn a_draft_sent_to_the_drive_is_stored_and_the_listing_shows_it() {
    let f = fixture().await;
    f.start_receiving();
    let bytes = payload(120_000);
    let file = pick(&f.dir, "季度报表.xlsx", &bytes).await;

    // 用户在面板里点了"云盘"。
    f.state().set_draft(vec![file.clone()]);
    let ids = f.state().queue_draft(Destination::Drive);
    assert_eq!(ids.len(), 1);
    until("the file to be stored in the drive", || {
        view_of(&f, &ids[0]).phase == TransferPhase::Complete
    })
    .await;

    let view = view_of(&f, &ids[0]);
    assert_eq!(view.direction, Direction::Out);
    assert_eq!(view.target, TransferTarget::Drive);
    assert_eq!(view.peer, "云盘");
    assert!(!view.retryable);
    assert_eq!(view.saved_as, None);

    // 存好之后名单换新了：界面看得到它，也看得到空间。
    // 名单是传完之后才去拿的，比传输记录晚一步：等它回来再数。
    until("the listing to show the stored file", || {
        listed(&f, "季度报表.xlsx")
    })
    .await;
    let listing = f
        .state()
        .snapshot()
        .drive
        .expect("the listing was refreshed after the upload");
    assert_eq!(
        listing
            .items
            .iter()
            .map(|item| item.name.clone())
            .collect::<Vec<_>>(),
        ["季度报表.xlsx"]
    );
    assert_eq!(listing.used_bytes, bytes.len() as u64);
    assert_eq!(listing.limit_bytes, f.server.state.config.drive_quota_bytes);
    let shown = serde_json::to_string(&listing).unwrap();
    assert!(!shown.contains("sha256"), "{shown}");

    // 服务端那边确实有了：另一台设备取得回，内容一字不差。
    let remote_id = &listing.items[0].id;
    assert_eq!(on_the_drive(&f, remote_id).await, bytes);
    // 存进云盘没有"等对方接收"的尾巴：没有活可领，也不欠回执。
    assert_eq!(f.state().next_job(), Next::Nothing);
    f.finish().await;
}

#[tokio::test]
async fn a_drive_file_fetched_here_lands_in_the_inbox_like_any_other_delivery() {
    let f = fixture().await;
    // 另一台设备先把文件存进了云盘。
    let bytes = payload(50_000);
    let file = pick(&f.dir, "合同.pdf", &bytes).await;
    let stored = f
        .sender
        .drive_upload("合同.pdf", &file, TransferOptions::new(&f.hold), None)
        .await
        .unwrap();
    refresh_drive(&f.state()).await;
    assert_eq!(
        f.state()
            .snapshot()
            .drive
            .map(|listing| listing.items.len()),
        Some(1)
    );
    let remote = f
        .state()
        .drive_file(&stored.id)
        .expect("the file is on the listing");

    // 用户点了"取回"。
    f.start_receiving();
    let id = f.state().fetch_from_drive(&remote);
    until("the file to be fetched", || {
        f.state().saved_file(&id).is_some()
    })
    .await;
    let saved = f.state().saved_file(&id).unwrap();
    assert_eq!(std::fs::read(&saved).unwrap(), bytes);
    assert_eq!(
        saved,
        std::fs::canonicalize(f.inbox().join("合同.pdf")).unwrap()
    );

    let view = view_of(&f, &id);
    assert_eq!(view.direction, Direction::In);
    assert_eq!(view.target, TransferTarget::Drive);
    assert_eq!(view.phase, TransferPhase::Complete);
    assert_eq!(view.name, "合同.pdf");
    // 通知登记过了，点它找得回文件；没有回执要补。
    assert_eq!(f.state().saved_file_for_handle(1), Some(saved));
    assert_eq!(f.state().next_job(), Next::Nothing);

    // 再取一次：不覆盖先前那份，加序号；记录还是同一条。
    assert_eq!(f.state().fetch_from_drive(&remote), id);
    until("the second copy to be fetched", || {
        view_of(&f, &id).phase == TransferPhase::Complete
            && view_of(&f, &id).saved_as.as_deref() == Some("合同 (2).pdf")
    })
    .await;
    assert_eq!(entries(&f.inbox()), ["合同 (2).pdf", "合同.pdf"]);
    assert_eq!(
        f.state()
            .snapshot()
            .transfers
            .iter()
            .filter(|t| t.id == id)
            .count(),
        1
    );
    f.finish().await;
}

#[tokio::test]
async fn a_drive_file_removed_before_it_is_fetched_is_reported_as_gone() {
    let f = fixture().await;
    let file = pick(&f.dir, "旧资料.pdf", &payload(800)).await;
    let stored = f
        .sender
        .drive_upload("旧资料.pdf", &file, TransferOptions::new(&f.hold), None)
        .await
        .unwrap();
    refresh_drive(&f.state()).await;
    let remote = f.state().drive_file(&stored.id).expect("on the listing");
    let id = f.state().fetch_from_drive(&remote);

    // 还没开始取，另一台设备把它从云盘里删了。
    f.sender
        .drive_delete(&stored.id, &RequestOptions::new(&f.hold))
        .await
        .unwrap();
    f.start_receiving();
    until("the disappearance to be recorded", || {
        view_of(&f, &id).phase == TransferPhase::Failed
    })
    .await;
    let view = view_of(&f, &id);
    assert_eq!(view.failure, Some(TransferFailure::Vanished));
    assert!(!view.retryable);
    // 接收目录为了这次取回建了出来，里面什么都没有。
    assert_eq!(entries(&f.inbox()), Vec::<String>::new());
    assert_eq!(f.state().saved_file(&id), None);
    // 这一条不会自己反复去取。
    assert_eq!(f.state().next_job(), Next::Nothing);
    f.finish().await;
}

#[tokio::test]
async fn a_drive_upload_that_does_not_fit_is_reported_and_a_smaller_one_still_fits() {
    let f = fixture_with_quota(Some(1_000)).await;
    f.start_receiving();
    let big = pick(&f.dir, "备份.zip", &payload(2_000)).await;
    let small = pick(&f.dir, "说明.txt", &payload(500)).await;

    f.state().queue_files(Destination::Drive, vec![big]);
    until("the refusal to be recorded", || {
        f.state()
            .snapshot()
            .transfers
            .iter()
            .any(|view| view.failure == Some(TransferFailure::QuotaExceeded))
    })
    .await;
    let refused = f
        .state()
        .snapshot()
        .transfers
        .into_iter()
        .find(|view| view.name == "备份.zip")
        .unwrap();
    assert_eq!(refused.phase, TransferPhase::Failed);
    // 腾出空间之后可以重发。
    assert!(refused.retryable);

    f.state().queue_files(Destination::Drive, vec![small]);
    until("the small file to be stored", || {
        f.state()
            .snapshot()
            .transfers
            .iter()
            .any(|view| view.name == "说明.txt" && view.phase == TransferPhase::Complete)
    })
    .await;
    // 名单比传输记录晚一步换新：等它回来。
    until("the listing to show the small file", || {
        listed(&f, "说明.txt")
    })
    .await;
    let listing = f
        .state()
        .snapshot()
        .drive
        .expect("the listing was refreshed");
    assert_eq!(
        listing
            .items
            .iter()
            .map(|item| item.name.clone())
            .collect::<Vec<_>>(),
        ["说明.txt"]
    );
    assert_eq!(listing.used_bytes, 500);
    f.finish().await;
}
