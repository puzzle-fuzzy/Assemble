//! 发送到设备的集成测试：对真实运行的服务走完整条链路。
//!
//! 两台"设备"都是真正的 `assemble_client::Client`，各自连着事件流（所以在线）。
//! 这里核对的是几方拼起来的行为：谁能在什么时候取内容、状态怎么变、
//! 文件什么时候从服务端消失、另一边什么时候收到通知。

mod common;

use std::time::Duration;

use assemble_client::{EventStream, OutgoingFile, TransferOptions};
use assemble_protocol::events::Event;
use assemble_protocol::primitives::{MAX_FILE_SIZE, Platform};
use assemble_protocol::sends::{Direction, FailReason, SendQuery, SendState};
use assemble_server::http::TestServer;
use assemble_store::secret::SecretString;
use common::*;
use reqwest::StatusCode;
use tokio_util::sync::CancellationToken;

// ---- 工具 ----

/// 一台连着事件流的设备。事件流必须一直被持有：放掉它，设备就离线了。
struct Online {
    device: Registered,
    events: EventStream,
}

impl Online {
    async fn join(fixture: &Fixture, seed: char, name: &str, platform: Platform) -> Self {
        let device = register(fixture, seed, name, platform).await;
        let mut events = device.open_events();
        wait_devices(&mut events, |devices| {
            online_of(devices, name).is_some_and(|device| device.online)
        })
        .await;
        Self { device, events }
    }
}

/// 两台都在线的设备：`pc` 发送，`mac` 接收。
async fn pair(fixture: &Fixture) -> (Online, Online) {
    let mut pc = Online::join(fixture, 'a', "工作电脑", Platform::Windows).await;
    let mac = Online::join(fixture, 'b', "MacBook", Platform::Macos).await;
    // `mac` 连上来时带的那份列表里已经有在线的 `pc`；`pc` 这边则会晚一些才收到
    // "MacBook 上线了"的通知，等它到了才算两台互相可见。
    wait_devices(&mut pc.events, |devices| {
        online_of(devices, "MacBook").is_some_and(|device| device.online)
    })
    .await;
    (pc, mac)
}

fn payload(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).expect("below 251"))
        .collect()
}

async fn outgoing(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> OutgoingFile {
    let path = dir.path().join(name);
    tokio::fs::write(&path, bytes).await.unwrap();
    OutgoingFile::open(&path, &CancellationToken::new())
        .await
        .unwrap()
}

/// 等到一条满足条件的发送事件。
async fn wait_send(
    events: &mut EventStream,
    satisfied: impl Fn(&assemble_protocol::sends::Send) -> bool,
) -> assemble_protocol::sends::Send {
    for _ in 0..40 {
        match next_event(events).await {
            Event::SendOffered { send } | Event::SendUpdated { send } if satisfied(&send) => {
                return send;
            }
            _ => {}
        }
    }
    panic!("no send event satisfied the condition")
}

async fn refcount(fixture: &Fixture, digest: &str) -> Option<i64> {
    sqlx::query_scalar::<_, i64>("SELECT refcount FROM blobs WHERE sha256 = ?")
        .bind(digest)
        .fetch_optional(&fixture.server.state.pool)
        .await
        .unwrap()
}

fn http(status: u16, code: &str) -> assemble_client::ClientError {
    assemble_client::ClientError::Http {
        status,
        remote_code: Some(code.to_owned()),
    }
}

// ---- 正常送达 ----

#[tokio::test]
async fn a_file_reaches_the_other_online_device_and_disappears_once_accepted() {
    let fixture = fixture().await;
    let (mut pc, mut mac) = pair(&fixture).await;
    let bytes = payload(300_000);
    let file = outgoing(&fixture.dir, "合同.pdf", &bytes).await;

    let offered = pc
        .device
        .send_to_device(
            &mac.device.device.id,
            &file,
            TransferOptions::new(&pc.device.cancel),
            None,
        )
        .await
        .unwrap();
    assert_eq!(offered.state, SendState::Offered);
    assert_eq!(offered.direction, Direction::Out);
    assert_eq!(offered.peer_device_name, "MacBook");
    assert_eq!(offered.name, "合同.pdf");
    assert_eq!(offered.size, bytes.len() as u64);
    assert!(offered.content_available());

    // 接收方收到通知：方向、对端都按它算。
    let incoming = wait_send(&mut mac.events, |send| send.id == offered.id).await;
    assert_eq!(incoming.direction, Direction::In);
    assert_eq!(incoming.peer_device_name, "工作电脑");
    assert_eq!(incoming.peer_device_id, pc.device.device.id);
    assert_eq!(incoming.sha256, file.sha256);
    // 事件只发给接收方：发送方的事件流里没有这条。
    assert_eq!(
        fixture
            .server
            .state
            .hub
            .listeners(&account_of(&fixture).await),
        2
    );

    // 内容在服务端，且只有一份。
    assert_eq!(refcount(&fixture, &file.sha256).await, Some(1));

    // 接收方取内容：原样、带文件名。
    let response = reqwest::Client::new()
        .get(format!(
            "{}/v1/sends/{}/content",
            fixture.server.origin.as_str(),
            offered.id
        ))
        .bearer_auth(mac.device.token.expose())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.content_length(), Some(bytes.len() as u64));
    assert_eq!(
        response.headers().get("content-disposition").unwrap(),
        "attachment; filename*=UTF-8''%E5%90%88%E5%90%8C.pdf"
    );
    assert_eq!(
        response.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert_eq!(response.bytes().await.unwrap(), bytes);

    // 收下并确认之后：状态变成已送达，文件从服务端消失。
    let delivered = mac
        .device
        .send_accept(&offered.id, &mac.device.options())
        .await
        .unwrap();
    assert_eq!(delivered.state, SendState::Delivered);
    assert!(!fixture.server.state.blobs.path_for(&file.sha256).exists());
    assert_eq!(refcount(&fixture, &file.sha256).await, None);

    // 发送方收到"已送达"。
    let update = wait_send(&mut pc.events, |send| {
        send.id == offered.id && send.state == SendState::Delivered
    })
    .await;
    assert_eq!(update.direction, Direction::Out);
    assert!(!update.content_available());

    // 再确认一次是幂等的：返回当前状态，而不是报冲突。
    let again = mac
        .device
        .send_accept(&offered.id, &mac.device.options())
        .await
        .unwrap();
    assert_eq!(again.state, SendState::Delivered);
}

#[tokio::test]
async fn content_can_be_fetched_in_ranges() {
    let fixture = fixture().await;
    let (pc, mac) = pair(&fixture).await;
    let bytes = payload(70_000);
    let file = outgoing(&fixture.dir, "a.bin", &bytes).await;
    let offered = pc
        .device
        .send_to_device(
            &mac.device.device.id,
            &file,
            TransferOptions::new(&pc.device.cancel),
            None,
        )
        .await
        .unwrap();

    let response = reqwest::Client::new()
        .get(format!(
            "{}/v1/sends/{}/content",
            fixture.server.origin.as_str(),
            offered.id
        ))
        .header("range", "bytes=1000-1999")
        .bearer_auth(mac.device.token.expose())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response.headers().get("content-range").unwrap(),
        &format!("bytes 1000-1999/{}", bytes.len())
    );
    assert_eq!(response.bytes().await.unwrap(), &bytes[1000..2000]);
}

#[tokio::test]
async fn identical_content_is_stored_once_however_many_times_it_is_sent() {
    let fixture = fixture().await;
    let (pc, mac) = pair(&fixture).await;
    let bytes = payload(50_000);
    let file = outgoing(&fixture.dir, "same.txt", &bytes).await;
    let first = pc
        .device
        .send_to_device(
            &mac.device.device.id,
            &file,
            TransferOptions::new(&pc.device.cancel),
            None,
        )
        .await
        .unwrap();
    let second = pc
        .device
        .send_to_device(
            &mac.device.device.id,
            &file,
            TransferOptions::new(&pc.device.cancel),
            None,
        )
        .await
        .unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(refcount(&fixture, &file.sha256).await, Some(2));

    // 放掉一个引用，文件还在；两个都放掉才删。
    mac.device
        .send_decline(&first.id, &mac.device.options())
        .await
        .unwrap();
    assert_eq!(refcount(&fixture, &file.sha256).await, Some(1));
    assert!(fixture.server.state.blobs.path_for(&file.sha256).exists());
    mac.device
        .send_accept(&second.id, &mac.device.options())
        .await
        .unwrap();
    assert!(!fixture.server.state.blobs.path_for(&file.sha256).exists());
}

// ---- 拒绝与校验 ----

#[tokio::test]
async fn a_device_that_is_offline_cannot_be_sent_to() {
    let fixture = fixture().await;
    let (mut pc, mac) = pair(&fixture).await;
    let bytes = payload(10_000);
    let file = outgoing(&fixture.dir, "notes.txt", &bytes).await;

    // 接收方放掉事件流：它下线了。
    drop(mac.events);
    wait_devices(&mut pc.events, |devices| {
        online_of(devices, "MacBook").is_some_and(|device| !device.online)
    })
    .await;

    let error = pc
        .device
        .send_to_device(
            &mac.device.device.id,
            &file,
            TransferOptions::new(&pc.device.cancel),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error, http(409, "DEVICE_OFFLINE"));
    // 什么都没留下。
    assert!(!fixture.server.state.blobs.path_for(&file.sha256).exists());
    assert_eq!(refcount(&fixture, &file.sha256).await, None);
    let listed = pc
        .device
        .sends(
            &SendQuery {
                direction: Direction::Out,
                state: None,
                limit: 10,
            },
            &pc.device.options(),
        )
        .await
        .unwrap();
    assert_eq!(listed, Vec::new());
}

/// 大到发送方不可能一口气塞进连接的那种文件。
fn bulk() -> Vec<u8> {
    // 同一段内容重复填充：比逐字节计算快得多，摘要照样是确定的。
    let block = payload(64 * 1024);
    block
        .iter()
        .copied()
        .cycle()
        .take(64 * 1024 * 1024)
        .collect()
}

/// 流式上传的大文件被提前拒绝时，发送方也要看得到原因。
///
/// 服务端不等正文读完就回绝，连接会在发送方还在发送时被关掉，发送方只会报
/// "连不上服务"。要复现它得满足两个条件，缺一个就复现不出来：
///
/// - 文件要大。小文件在回绝之前就已经全部进了连接的缓冲区，连接层自己就把剩下的读完了；
/// - 服务端要真的与发送方同时运行。单线程的测试运行时里，发送方会一口气写到写不动为止，
/// 服务端才有机会出手。所以这里用多线程的运行时。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_streaming_sender_is_told_why_a_large_upload_was_refused() {
    let fixture = fixture().await;
    let (mut pc, mac) = pair(&fixture).await;
    let file = outgoing(&fixture.dir, "big.bin", &bulk()).await;

    // 对方下线了，发送方手里还是一个大文件。
    drop(mac.events);
    wait_devices(&mut pc.events, |devices| {
        online_of(devices, "MacBook").is_some_and(|device| !device.online)
    })
    .await;
    let error = pc
        .device
        .send_to_device(
            &mac.device.device.id,
            &file,
            TransferOptions::new(&pc.device.cancel),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error, http(409, "DEVICE_OFFLINE"));

    // 给自己发：同样说得出原因，而不是"连不上服务"。
    let error = pc
        .device
        .send_to_device(
            &pc.device.device.id,
            &file,
            TransferOptions::new(&pc.device.cancel),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error, http(400, "INVALID_REQUEST"));

    // 两次的正文都丢掉了，没有落盘。
    assert!(!fixture.server.state.blobs.path_for(&file.sha256).exists());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM sends")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn sends_are_refused_before_any_content_is_stored() {
    let fixture = fixture().await;
    let (pc, mac) = pair(&fixture).await;
    let bytes = payload(1_000);
    let file = outgoing(&fixture.dir, "ok.bin", &bytes).await;
    let base = fixture.server.origin.as_str();
    let http = reqwest::Client::new();
    let token = pc.device.token.expose();

    // 摘要声明得不对：内容收完了也拒绝，什么都不落盘。
    let response = http
        .post(format!(
            "{base}/v1/sends?toDeviceId={}&name=x.bin&size={}&sha256={}",
            mac.device.device.id,
            bytes.len(),
            "0".repeat(64)
        ))
        .bearer_auth(token)
        .body(bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "HASH_MISMATCH"
    );
    assert!(
        !fixture
            .server
            .state
            .blobs
            .path_for(&"0".repeat(64))
            .exists()
    );

    // 声明的长度超过单文件上限：不必读正文就拒绝。
    let response = http
        .post(format!(
            "{base}/v1/sends?toDeviceId={}&name=x.bin&size={}&sha256={}",
            mac.device.device.id,
            MAX_FILE_SIZE + 1,
            file.sha256
        ))
        .bearer_auth(token)
        .body(bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // 给自己发、给不存在的设备发、给别的账户的设备发。
    let response = http
        .post(format!(
            "{base}/v1/sends?toDeviceId={}&name=x.bin&size={}&sha256={}",
            pc.device.device.id,
            bytes.len(),
            file.sha256
        ))
        .bearer_auth(token)
        .body(bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = http
        .post(format!(
            "{base}/v1/sends?toDeviceId=missing&name=x.bin&size={}&sha256={}",
            bytes.len(),
            file.sha256
        ))
        .bearer_auth(token)
        .body(bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // 名字不能是个路径，摘要得像摘要，媒体类型得像媒体类型。
    for query in [
        format!(
            "toDeviceId={}&name=a%2Fb&size=1&sha256={}",
            mac.device.device.id, file.sha256
        ),
        format!(
            "toDeviceId={}&name=b&size=1&sha256=nope",
            mac.device.device.id
        ),
        format!(
            "toDeviceId={}&name=b&size=1&sha256={}&mimeType=bad%20mime",
            mac.device.device.id, file.sha256
        ),
    ] {
        let response = http
            .post(format!("{base}/v1/sends?{query}"))
            .bearer_auth(token)
            .body(bytes.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
    }

    assert!(!fixture.server.state.blobs.path_for(&file.sha256).exists());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM sends")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

// ---- 取内容的权限 ----

#[tokio::test]
async fn only_the_receiver_can_fetch_the_content_and_only_while_it_is_offered() {
    let fixture = fixture().await;
    let (pc, mac) = pair(&fixture).await;
    let bytes = payload(2_000);
    let file = outgoing(&fixture.dir, "secret.txt", &bytes).await;
    let offered = pc
        .device
        .send_to_device(
            &mac.device.device.id,
            &file,
            TransferOptions::new(&pc.device.cancel),
            None,
        )
        .await
        .unwrap();
    let url = format!(
        "{}/v1/sends/{}/content",
        fixture.server.origin.as_str(),
        offered.id
    );
    let http = reqwest::Client::new();

    // 没带令牌、带发送方的令牌：都不行。
    assert_eq!(
        http.get(&url).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        http.get(&url)
            .bearer_auth(pc.device.token.expose())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );

    // 发送方撤回之后，接收方也取不到了。
    let cancelled = pc
        .device
        .send_cancel(&offered.id, &pc.device.options())
        .await
        .unwrap();
    assert_eq!(cancelled.state, SendState::Cancelled);
    let response = http
        .get(&url)
        .bearer_auth(mac.device.token.expose())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "SEND_STATE_CONFLICT"
    );
    assert!(!fixture.server.state.blobs.path_for(&file.sha256).exists());
}

// ---- 终态与通知 ----

#[tokio::test]
async fn declining_tells_the_sender_and_releases_the_content() {
    let fixture = fixture().await;
    let (mut pc, mac) = pair(&fixture).await;
    let bytes = payload(4_000);
    let file = outgoing(&fixture.dir, "report.pdf", &bytes).await;
    let offered = pc
        .device
        .send_to_device(
            &mac.device.device.id,
            &file,
            TransferOptions::new(&pc.device.cancel),
            None,
        )
        .await
        .unwrap();

    let declined = mac
        .device
        .send_decline(&offered.id, &mac.device.options())
        .await
        .unwrap();
    assert_eq!(declined.state, SendState::Declined);
    let update = wait_send(&mut pc.events, |send| send.id == offered.id).await;
    assert_eq!(update.state, SendState::Declined);
    assert_eq!(update.direction, Direction::Out);
    assert!(!fixture.server.state.blobs.path_for(&file.sha256).exists());

    // 已经拒绝了，就不能再收下。
    let error = mac
        .device
        .send_accept(&offered.id, &mac.device.options())
        .await
        .unwrap_err();
    assert_eq!(error, http(409, "SEND_STATE_CONFLICT"));
}

#[tokio::test]
async fn a_failed_receive_is_reported_to_the_sender_with_its_reason() {
    let fixture = fixture().await;
    let (mut pc, mac) = pair(&fixture).await;
    let bytes = payload(4_000);
    let file = outgoing(&fixture.dir, "big.mov", &bytes).await;
    let offered = pc
        .device
        .send_to_device(
            &mac.device.device.id,
            &file,
            TransferOptions::new(&pc.device.cancel),
            None,
        )
        .await
        .unwrap();

    let failed = mac
        .device
        .send_failed(
            &offered.id,
            assemble_protocol::sends::FailInput {
                reason: FailReason::DiskFull,
            },
            &mac.device.options(),
        )
        .await
        .unwrap();
    assert_eq!(failed.state, SendState::Failed);
    assert_eq!(failed.failure, Some(FailReason::DiskFull));

    let update = wait_send(&mut pc.events, |send| send.id == offered.id).await;
    assert_eq!(update.state, SendState::Failed);
    assert_eq!(update.failure, Some(FailReason::DiskFull));
    assert!(!fixture.server.state.blobs.path_for(&file.sha256).exists());
}

#[tokio::test]
async fn cancelling_tells_the_receiver() {
    let fixture = fixture().await;
    let (pc, mut mac) = pair(&fixture).await;
    let bytes = payload(3_000);
    let file = outgoing(&fixture.dir, "draft.docx", &bytes).await;
    let offered = pc
        .device
        .send_to_device(
            &mac.device.device.id,
            &file,
            TransferOptions::new(&pc.device.cancel),
            None,
        )
        .await
        .unwrap();
    // 先把"有文件到了"那条事件读掉。
    wait_send(&mut mac.events, |send| send.id == offered.id).await;

    pc.device
        .send_cancel(&offered.id, &pc.device.options())
        .await
        .unwrap();
    let update = wait_send(&mut mac.events, |send| send.id == offered.id).await;
    assert_eq!(update.state, SendState::Cancelled);
    assert_eq!(update.direction, Direction::In);
}

#[tokio::test]
async fn only_the_right_side_can_settle_a_send() {
    let fixture = fixture().await;
    let (pc, mac) = pair(&fixture).await;
    let bytes = payload(500);
    let file = outgoing(&fixture.dir, "ping.txt", &bytes).await;
    let offered = pc
        .device
        .send_to_device(
            &mac.device.device.id,
            &file,
            TransferOptions::new(&pc.device.cancel),
            None,
        )
        .await
        .unwrap();

    // 发送方不能替接收方"收下"或"报告失败"；接收方不能替发送方"撤回"。
    let error = pc
        .device
        .send_accept(&offered.id, &pc.device.options())
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(403));
    let error = pc
        .device
        .send_failed(
            &offered.id,
            assemble_protocol::sends::FailInput {
                reason: FailReason::WriteFailed,
            },
            &pc.device.options(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(403));
    let error = mac
        .device
        .send_cancel(&offered.id, &mac.device.options())
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(403));

    // 被拒的几次尝试没有改动任何东西：这次发送仍在等待接收，内容还在。
    let waiting = pc
        .device
        .sends(
            &SendQuery {
                direction: Direction::Out,
                state: Some(SendState::Offered),
                limit: 10,
            },
            &pc.device.options(),
        )
        .await
        .unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(refcount(&fixture, &file.sha256).await, Some(1));
}

// ---- 列表 ----

#[tokio::test]
async fn listings_depend_on_who_is_asking_and_filter_by_state() {
    let fixture = fixture().await;
    let (pc, mac) = pair(&fixture).await;
    let bytes = payload(100);
    let file = outgoing(&fixture.dir, "list.txt", &bytes).await;
    let first = pc
        .device
        .send_to_device(
            &mac.device.device.id,
            &file,
            TransferOptions::new(&pc.device.cancel),
            None,
        )
        .await
        .unwrap();
    let second = pc
        .device
        .send_to_device(
            &mac.device.device.id,
            &file,
            TransferOptions::new(&pc.device.cancel),
            None,
        )
        .await
        .unwrap();
    mac.device
        .send_accept(&first.id, &mac.device.options())
        .await
        .unwrap();

    let outgoing = pc
        .device
        .sends(
            &SendQuery {
                direction: Direction::Out,
                state: None,
                limit: 50,
            },
            &pc.device.options(),
        )
        .await
        .unwrap();
    // 新的在前。
    assert_eq!(
        outgoing
            .iter()
            .map(|send| send.id.as_str())
            .collect::<Vec<_>>(),
        vec![second.id.as_str(), first.id.as_str()]
    );
    assert!(outgoing.iter().all(|send| send.direction == Direction::Out));

    let waiting = pc
        .device
        .sends(
            &SendQuery {
                direction: Direction::Out,
                state: Some(SendState::Offered),
                limit: 50,
            },
            &pc.device.options(),
        )
        .await
        .unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].id, second.id);

    let incoming = mac
        .device
        .sends(
            &SendQuery {
                direction: Direction::In,
                state: None,
                limit: 1,
            },
            &mac.device.options(),
        )
        .await
        .unwrap();
    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0].id, second.id);
    assert_eq!(incoming[0].direction, Direction::In);
}

// ---- 过期与清理 ----

#[tokio::test]
async fn sends_nobody_accepts_expire_and_release_their_content() {
    let dir = tempfile::tempdir().unwrap();
    let pool = assemble_server::db::open(dir.path()).await.unwrap();
    // 时限为零：任何发送都算"过了时限还没送达"。
    let config = assemble_server::Config::development(dir.path());
    let config = assemble_server::Config {
        send_ttl: Duration::ZERO,
        ..config
    };
    let server = TestServer::start(config, pool).await.unwrap();
    let fixture = Fixture {
        browser: Browser::new(server.origin.as_str()),
        server,
        dir,
    };
    assemble_server::accounts::create(
        &fixture.server.state.pool,
        USERNAME,
        "",
        SecretString::new(PASSWORD.to_owned()),
    )
    .await
    .unwrap();
    let (mut pc, mut mac) = pair(&fixture).await;
    let bytes = payload(9_000);
    let file = outgoing(&fixture.dir, "later.zip", &bytes).await;
    let offered = pc
        .device
        .send_to_device(
            &mac.device.device.id,
            &file,
            TransferOptions::new(&pc.device.cancel),
            None,
        )
        .await
        .unwrap();
    wait_send(&mut mac.events, |send| send.id == offered.id).await;

    // 时限是按毫秒比较的："建立时刻"早于"现在减去时限"才算过期。记录刚建好的那一毫秒里
    // 两者相等，还不算过期。等时钟走过这一毫秒再清理，结果才每次都一样。
    tokio::time::sleep(Duration::from_millis(5)).await;
    let report = assemble_server::maintenance::run_once(&fixture.server.state)
        .await
        .unwrap();
    assert_eq!(report.expired_sends, 1);
    assert!(!fixture.server.state.blobs.path_for(&file.sha256).exists());
    assert_eq!(refcount(&fixture, &file.sha256).await, None);

    // 两边都收到"已过期"。
    let update = wait_send(&mut pc.events, |send| send.id == offered.id).await;
    assert_eq!(update.state, SendState::Expired);
    assert_eq!(update.direction, Direction::Out);
    let update = wait_send(&mut mac.events, |send| send.id == offered.id).await;
    assert_eq!(update.state, SendState::Expired);
    assert_eq!(update.direction, Direction::In);

    // 再清理一轮不会重复计数。
    let again = assemble_server::maintenance::run_once(&fixture.server.state)
        .await
        .unwrap();
    assert_eq!(again.expired_sends, 0);

    // 过期的发送不能再被收下。
    let error = mac
        .device
        .send_accept(&offered.id, &mac.device.options())
        .await
        .unwrap_err();
    assert_eq!(error, http(409, "SEND_STATE_CONFLICT"));
}
