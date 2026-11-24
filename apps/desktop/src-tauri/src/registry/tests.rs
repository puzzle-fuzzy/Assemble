//! 传输记录表的单元测试：不碰网络、文件与窗口，只看它记得对不对、下一步挑得对不对。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use assemble_client::OutgoingFile;
use assemble_protocol::drive::DriveFile;
use assemble_protocol::primitives::Timestamp;
use assemble_protocol::sends::{Direction, FailReason, Send as SendRecord, SendState};

use super::*;
use crate::state::DeviceView;

/// 发送方那边的记录：这台设备发给 `mac` 的一个文件。
fn offered(id: &str) -> SendRecord {
    SendRecord {
        id: id.into(),
        direction: Direction::Out,
        peer_device_id: "mac".into(),
        peer_device_name: "Mac".into(),
        name: "合同.pdf".into(),
        size: 1_000,
        mime_type: None,
        sha256: "a".repeat(64),
        state: SendState::Offered,
        failure: None,
        created_at: Timestamp::now(),
        updated_at: Timestamp::now(),
    }
}

/// 同一次发送，换成收件方看的样子。
fn incoming(id: &str) -> SendRecord {
    let mut send = offered(id);
    send.direction = Direction::In;
    send.peer_device_id = "pc".into();
    send.peer_device_name = "工作电脑".into();
    send
}

fn settled(id: &str, state: SendState, failure: Option<FailReason>) -> SendRecord {
    let mut send = offered(id);
    send.state = state;
    send.failure = failure;
    send
}

fn file(name: &str, size: u64, sha: &str) -> OutgoingFile {
    OutgoingFile {
        path: PathBuf::from(format!("/private/var/tmp/{name}")),
        size,
        sha256: sha.to_owned(),
    }
}

fn target() -> DeviceView {
    DeviceView {
        id: "pc".into(),
        name: "工作电脑".into(),
        platform: "windows".into(),
        online: true,
    }
}

/// 挑两个文件，排进队列。返回两条记录的标识，先挑的在前。
fn queued(registry: &mut Registry) -> (String, String) {
    registry.set_draft(vec![
        file("合同.pdf", 10, &"a".repeat(64)),
        file("照片.jpg", 20, &"b".repeat(64)),
    ]);
    let mut ids = registry.queue_draft(Destination::Device(target()));
    (ids.remove(0), ids.remove(0))
}

fn view_of(registry: &Registry, id: &str) -> TransferView {
    registry
        .views()
        .into_iter()
        .find(|view| view.id == id)
        .unwrap_or_else(|| panic!("{id} is not listed"))
}

fn nearly(left: f64, right: f64) -> bool {
    (left - right).abs() < 1e-9
}

#[test]
fn the_worker_takes_files_in_order_receiving_first_and_only_when_they_are_due() {
    let now = Instant::now();
    let mut registry = Registry::default();
    registry.offer(&incoming("a"));
    registry.offer(&incoming("b"));
    let (first, _) = queued(&mut registry);

    // 记录是新的在前；倒着看才是先来后到。收文件排在发文件前面：对面正等着。
    assert_eq!(
        registry.next_job(now),
        Next::Work(Job::Receive { id: "a".into() })
    );
    assert!(registry.begin_receive("a").is_some());
    assert_eq!(
        registry.next_job(now),
        Next::Work(Job::Receive { id: "b".into() })
    );
    assert!(registry.begin_receive("b").is_some());
    assert_eq!(
        registry.next_job(now),
        Next::Work(Job::Send { id: first.clone() })
    );
    assert!(registry.begin_send(&first).is_some());
    // 先挑的先发；正在传的不算待办。
    assert_eq!(
        registry.next_job(now),
        Next::Work(Job::Send { id: "out-2".into() })
    );
    assert!(registry.begin_send("out-2").is_some());
    assert_eq!(registry.next_job(now), Next::Nothing);

    // 还没到点的重试不算活，但要说得出多久之后有。
    registry.retry_send(&first, now);
    assert_eq!(registry.next_job(now), Next::Wait(Duration::from_secs(2)));
    assert_eq!(
        registry.next_job(now + Duration::from_secs(2)),
        Next::Work(Job::Send { id: first.clone() })
    );
    assert!(registry.begin_send(&first).is_some());
    assert_eq!(
        registry.next_job(now + Duration::from_secs(60)),
        Next::Nothing
    );
}

#[test]
fn receipts_go_out_after_the_files_and_stop_once_the_server_knows() {
    let now = Instant::now();
    let mut registry = Registry::default();
    registry.offer(&incoming("a"));
    registry.set_draft(vec![file("x.bin", 1, &"c".repeat(64))]);
    let send = registry.queue_draft(Destination::Device(target()))[0].clone();
    // 先收文件，再发文件，最后补回执。
    assert!(matches!(
        registry.next_job(now),
        Next::Work(Job::Receive { .. })
    ));
    assert!(registry.begin_receive("a").is_some());
    // 队列里还有个文件没发：它排在回执前面。
    assert_eq!(
        registry.next_job(now),
        Next::Work(Job::Send { id: send.clone() })
    );
    assert!(registry.begin_send(&send).is_some());

    registry.receive_completed("a", "合同.pdf", PathBuf::from("/x"), 1);
    assert_eq!(
        registry.next_job(now),
        Next::Work(Job::Settle { id: "a".into() })
    );
    assert_eq!(
        registry
            .settlement_of("a")
            .map(|(send, report)| (send.id, report)),
        Some(("a".into(), None))
    );

    // 服务端已经知道结局：不再补。正在发的那个也不算待办。
    registry.settled("a");
    assert_eq!(registry.settlement_of("a"), None);
    assert_eq!(registry.next_job(now), Next::Nothing);

    // 撤回、过期的记录没有回执可补。
    for state in [SendState::Cancelled, SendState::Expired] {
        let mut registry = Registry::default();
        registry.offer(&incoming("d"));
        registry.updated(&settled("d", state, None));
        assert_eq!(registry.next_job(now), Next::Nothing, "{state:?}");
        assert_eq!(registry.settlement_of("d"), None);
    }
}

#[test]
fn a_file_the_server_still_waits_for_is_continued_rather_than_started_over() {
    let mut registry = Registry::default();
    registry.offer(&incoming("a"));
    assert!(registry.begin_receive("a").is_some());
    registry.progress("a", 0.4);
    // 正在接收：接着收，进度不动。
    registry.offer(&incoming("a"));
    assert_eq!(view_of(&registry, "a").phase, TransferPhase::InProgress);
    assert!(nearly(view_of(&registry, "a").progress, 0.4));

    // 文件存好了、服务端却还在等：说明回执没送到，清掉退避再发一次。
    registry.receive_completed("a", "合同.pdf", PathBuf::from("/x"), 1);
    registry.settled("a");
    registry.retry_settle("a", Instant::now());
    registry.offer(&incoming("a"));
    assert_eq!(
        registry.settlement_of("a").map(|(send, _)| send.id),
        Some("a".into())
    );
    assert!(matches!(
        registry.next_job(Instant::now()),
        Next::Work(Job::Settle { id }) if id == "a"
    ));

    // 本机以为对面撤回了，服务端却还在等：以服务端为准，重新收。
    let mut registry = Registry::default();
    registry.offer(&incoming("a"));
    registry.updated(&settled("a", SendState::Cancelled, None));
    assert_eq!(view_of(&registry, "a").phase, TransferPhase::Cancelled);
    registry.offer(&incoming("a"));
    assert_eq!(view_of(&registry, "a").phase, TransferPhase::Queued);
}

#[test]
fn waiting_between_retries_gets_longer_up_to_five_minutes() {
    assert_eq!(backoff(0), Duration::from_secs(2));
    assert_eq!(backoff(1), Duration::from_secs(2));
    assert_eq!(backoff(3), Duration::from_secs(15));
    assert_eq!(backoff(4), Duration::from_secs(60));
    assert_eq!(backoff(40), Duration::from_secs(5 * 60));
}

#[test]
fn an_update_about_an_unknown_send_is_ignored() {
    let mut registry = Registry::default();
    assert_eq!(
        registry.updated(&settled("s-404", SendState::Delivered, None)),
        Updated::Ignored
    );
    assert_eq!(registry.views(), Vec::new());
}

// ---- 发出的 ----

#[test]
fn a_draft_shows_names_and_sizes_but_never_a_location() {
    let mut registry = Registry::default();
    let view = registry.set_draft(vec![file("季度报表.xlsx", 524_288, &"a".repeat(64))]);
    assert_eq!(view.id, "draft-1");
    assert_eq!(view.files.len(), 1);
    assert_eq!(view.files[0].name, "季度报表.xlsx");
    assert_eq!(view.files[0].size, 524_288);
    let shown = serde_json::to_string(&registry.draft_view()).unwrap();
    assert!(!shown.contains("/private/var"), "{shown}");

    // 再挑一次就是另一份草稿，之前挑的作废。
    let view = registry.set_draft(vec![file("说明.txt", 2_048, &"b".repeat(64))]);
    assert_eq!(view.id, "draft-2");
    assert_eq!(
        registry.draft_view().map(|draft| draft.files.len()),
        Some(1)
    );
    registry.drop_draft();
    assert!(registry.draft_view().is_none());
    // 没有草稿时发起发送：什么都不排。
    assert_eq!(
        registry.queue_draft(Destination::Device(target())),
        Vec::<String>::new()
    );
}

#[test]
fn queuing_a_draft_turns_each_file_into_a_record_and_consumes_the_draft() {
    let mut registry = Registry::default();
    let (first, second) = queued(&mut registry);
    assert_eq!((first.as_str(), second.as_str()), ("out-1", "out-2"));
    assert!(registry.draft_view().is_none());

    let views = registry.views();
    assert_eq!(views.len(), 2);
    // 新的在前：后挑的那个排最上面，但先挑的先发。
    let later = &views[0];
    assert_eq!(later.id, "out-2");
    assert_eq!(later.direction, Direction::Out);
    assert_eq!(later.peer, "工作电脑");
    assert_eq!(later.name, "照片.jpg");
    assert_eq!(later.size, 20);
    assert_eq!(later.phase, TransferPhase::Queued);
    assert!(!later.retryable);
    // 记录里留着位置，但那一份不交给界面。
    let shown = serde_json::to_string(&views).unwrap();
    assert!(!shown.contains("/private/var"), "{shown}");
}

#[test]
fn a_sent_file_waits_for_the_other_device_and_then_takes_its_verdict() {
    let mut registry = Registry::default();
    let (first, _) = queued(&mut registry);
    let outgoing = registry.begin_send(&first).expect("queued");
    assert!(matches!(
        &outgoing.destination,
        Destination::Device(device) if device.name == "工作电脑"
    ));
    assert_eq!(outgoing.file.name(), "合同.pdf");
    assert!(outgoing.send.is_none());
    assert_eq!(view_of(&registry, &first).phase, TransferPhase::InProgress);
    // 领过一次就不会再领。
    assert!(registry.begin_send(&first).is_none());

    let record = offered("s-1");
    assert_eq!(registry.send_uploaded(&first, &record), Updated::Applied);
    assert_eq!(view_of(&registry, &first).phase, TransferPhase::Waiting);
    assert!(nearly(view_of(&registry, &first).progress, 1.0));
    // 等对方接收的记录没有活可领：剩下的只有队列里另一个还没发的文件。
    assert!(matches!(
        registry.next_job(Instant::now()),
        Next::Work(Job::Send { id }) if id == "out-2"
    ));

    for (state, failure, expected) in [
        (SendState::Delivered, None, TransferPhase::Complete),
        (SendState::Declined, None, TransferPhase::Declined),
        (SendState::Cancelled, None, TransferPhase::Cancelled),
        (SendState::Expired, None, TransferPhase::Expired),
        (
            SendState::Failed,
            Some(FailReason::DiskFull),
            TransferPhase::Failed,
        ),
    ] {
        let mut registry = Registry::default();
        let (first, _) = queued(&mut registry);
        registry.begin_send(&first);
        registry.send_uploaded(&first, &record);
        // 服务端按它那边的标识通知结局；本机的记录编号不一样，靠存下的那条对上。
        assert_eq!(
            registry.updated(&settled("s-1", state, failure)),
            Updated::Applied,
            "{state:?}"
        );
        let view = view_of(&registry, &first);
        assert_eq!(view.phase, expected, "{state:?}");
        assert_eq!(view.failure, failure.map(TransferFailure::from));
        // 文件到过服务端：对面拒收、过期、写不下来，本机重发都没有意义。
        assert!(!view.retryable, "{state:?}");
    }
}

#[test]
fn a_send_refused_before_the_file_reached_the_server_can_be_tried_again() {
    let now = Instant::now();
    let mut registry = Registry::default();
    let (first, second) = queued(&mut registry);
    registry.begin_send(&first);
    registry.send_failed(&first, TransferFailure::DeviceOffline);
    let view = view_of(&registry, &first);
    assert_eq!(view.phase, TransferPhase::Failed);
    assert_eq!(view.failure, Some(TransferFailure::DeviceOffline));
    assert!(view.retryable);
    // 失败得等用户决定，不自己重试。
    assert!(matches!(registry.next_job(now), Next::Work(Job::Send { id }) if id == second));

    // 另一个文件到了服务端，是对方写不下来：那条不能重发。
    registry.begin_send(&second);
    registry.send_uploaded(&second, &offered("s-9"));
    registry.updated(&settled(
        "s-9",
        SendState::Failed,
        Some(FailReason::WriteFailed),
    ));
    assert!(!view_of(&registry, &second).retryable);
    assert!(!registry.retry_failed_send(&second, now));

    assert!(registry.retry_failed_send(&first, now));
    assert_eq!(view_of(&registry, &first).phase, TransferPhase::Queued);
    assert_eq!(view_of(&registry, &first).failure, None);
    assert!(registry.begin_send(&first).is_some());
}

#[test]
fn cancelling_leaves_the_server_alone_until_the_file_has_arrived_there() {
    let now = Instant::now();
    let mut registry = Registry::default();
    let (first, second) = queued(&mut registry);
    // 还没轮到它：到此为止，队列里不再有它。
    assert_eq!(registry.cancel(&second), Cancellation::Local);
    assert_eq!(view_of(&registry, &second).phase, TransferPhase::Cancelled);
    assert_eq!(
        registry.next_job(now),
        Next::Work(Job::Send { id: first.clone() })
    );
    // 发了一半：同样到此为止，服务端那边什么都没有。
    registry.begin_send(&first);
    assert_eq!(registry.cancel(&first), Cancellation::Local);
    assert_eq!(view_of(&registry, &first).phase, TransferPhase::Cancelled);
    assert_eq!(registry.next_job(now), Next::Nothing);
    assert_eq!(registry.cancel("missing"), Cancellation::Nothing);

    // 文件已经在服务端等对方接收：要去服务端撤回，本机先不动。
    let (third, _) = queued(&mut registry);
    registry.begin_send(&third);
    registry.send_uploaded(&third, &offered("s-1"));
    assert_eq!(
        registry.cancel(&third),
        Cancellation::OnServer("s-1".into())
    );
    assert_eq!(view_of(&registry, &third).phase, TransferPhase::Waiting);
    // 撤回之后服务端说"已撤回"。
    registry.updated(&settled("s-1", SendState::Cancelled, None));
    assert_eq!(view_of(&registry, &third).phase, TransferPhase::Cancelled);
    // 到头了就没什么可取消的。
    assert_eq!(registry.cancel(&third), Cancellation::Nothing);

    // 收到的文件：停下来，并把"用户取消了"报告给发送方。
    let mut registry = Registry::default();
    registry.offer(&incoming("a"));
    registry.begin_receive("a");
    assert_eq!(registry.cancel("a"), Cancellation::Local);
    assert_eq!(view_of(&registry, "a").phase, TransferPhase::Failed);
    assert_eq!(
        registry.settlement_of("a").map(|(_, report)| report),
        Some(Some(ReceiveFailure::Cancelled))
    );
    // 已经存好的文件撤不回：这时取消什么都不做。
    let mut registry = Registry::default();
    registry.offer(&incoming("a"));
    registry.begin_receive("a");
    registry.receive_completed("a", "合同.pdf", PathBuf::from("/x"), 1);
    assert_eq!(registry.cancel("a"), Cancellation::Nothing);
    assert_eq!(view_of(&registry, "a").phase, TransferPhase::Complete);
}

#[test]
fn an_upload_abandoned_halfway_must_be_withdrawn_if_it_turns_out_to_have_arrived() {
    let mut registry = Registry::default();
    let (first, _) = queued(&mut registry);
    registry.begin_send(&first);
    registry.cancel(&first);

    // 服务端其实已经收下了：记下它给的那条，并且要去撤回；记录不改回"等对方接收"。
    let record = offered("s-1");
    assert_eq!(registry.send_uploaded(&first, &record), Updated::Withdraw);
    assert_eq!(view_of(&registry, &first).phase, TransferPhase::Cancelled);
    // 已经放弃的那次发送又来了一次"还在等"的通知：还是要撤回。
    assert_eq!(registry.updated(&record), Updated::Withdraw);
    // 撤回之后服务端说"已撤回"：记上，不再要求撤回。
    assert_eq!(
        registry.updated(&settled("s-1", SendState::Cancelled, None)),
        Updated::Applied
    );
    assert!(!registry.abandoned(&record));
}

#[test]
fn an_abandoned_upload_is_recognised_on_the_server_by_what_it_carries() {
    let mut registry = Registry::default();
    let (first, _) = queued(&mut registry);
    registry.begin_send(&first);
    registry.cancel(&first);

    // 服务端那边的记录：发件人是这台设备，收件人是 `pc`。
    let mut on_server = offered("s-1");
    on_server.sha256 = "a".repeat(64);
    on_server.size = 10;
    on_server.peer_device_id = "pc".into();
    assert!(registry.abandoned(&on_server));

    // 内容、名字、收件人有一个不一样，就不是它。
    let mut other = on_server.clone();
    other.sha256 = "f".repeat(64);
    assert!(!registry.abandoned(&other));
    let mut other = on_server.clone();
    other.name = "照片.jpg".into();
    assert!(!registry.abandoned(&other));
    let mut other = on_server.clone();
    other.peer_device_id = "mac".into();
    assert!(!registry.abandoned(&other));
    // 没放弃过的发送不算。
    let mut registry = Registry::default();
    let (first, _) = queued(&mut registry);
    registry.begin_send(&first);
    assert!(!registry.abandoned(&on_server));
}

#[test]
fn progress_counts_only_whole_percentages_and_only_while_a_transfer_runs() {
    let mut registry = Registry::default();
    registry.offer(&incoming("a"));
    registry.progress("a", 0.5);
    assert!(nearly(view_of(&registry, "a").progress, 0.0));
    registry.begin_receive("a");
    registry.progress("a", 0.009);
    assert!(nearly(view_of(&registry, "a").progress, 0.0));
    registry.progress("a", 0.01);
    assert!(nearly(view_of(&registry, "a").progress, 0.01));
    registry.progress("a", 7.0);
    assert!(nearly(view_of(&registry, "a").progress, 1.0));
}

#[test]
fn only_the_oldest_finished_records_are_dropped_when_the_list_grows_too_long() {
    let mut registry = Registry::default();
    for index in 0..MAX_TRANSFERS {
        let id = format!("f{index}");
        registry.offer(&incoming(&id));
        registry.updated(&settled(&id, SendState::Delivered, None));
    }
    assert_eq!(registry.views().len(), MAX_TRANSFERS);
    registry.offer(&incoming("new"));
    assert_eq!(registry.views().len(), MAX_TRANSFERS);
    // 最早的那条被丢掉；新来的和其余的都在。
    assert!(registry.views().iter().all(|view| view.id != "f0"));
    assert!(registry.views().iter().any(|view| view.id == "new"));
    assert!(registry.views().iter().any(|view| view.id == "f99"));

    // 全是没结束的记录时一条也不丢：它们都还等着办事。
    let mut registry = Registry::default();
    for index in 0..MAX_TRANSFERS + 5 {
        registry.offer(&incoming(&format!("q{index}")));
    }
    assert_eq!(registry.views().len(), MAX_TRANSFERS + 5);
}

#[test]
fn clearing_forgets_everything_that_belongs_to_the_account() {
    let mut registry = Registry::default();
    registry.offer(&incoming("a"));
    queued(&mut registry);
    registry.clear();
    assert_eq!(registry.views(), Vec::new());
    assert!(registry.draft_view().is_none());
    assert_eq!(registry.next_job(Instant::now()), Next::Nothing);
    // 清过之后编号接着往下走，不会与界面还留着的旧标识撞上。
    let (first, _) = queued(&mut registry);
    assert_eq!(first, "out-3");
    // 发起之后草稿就用掉了；再挑一份，编号接着往下走。
    assert!(registry.draft_view().is_none());
    let draft = registry.set_draft(vec![file("说明.txt", 2_048, &"b".repeat(64))]);
    assert_eq!(draft.id, "draft-3");
}

#[test]
fn every_failure_the_server_can_report_has_a_word_and_a_way_back() {
    for reason in [
        FailReason::DiskFull,
        FailReason::WriteFailed,
        FailReason::VerificationFailed,
        FailReason::Cancelled,
    ] {
        let failure = TransferFailure::from(reason);
        assert_eq!(failure.reported(), Some(reason_to_receive(reason)));
    }
    for only_sent in [
        TransferFailure::DeviceOffline,
        TransferFailure::TooLarge,
        TransferFailure::FileUnreadable,
        TransferFailure::Rejected,
    ] {
        assert_eq!(only_sent.reported(), None);
    }
}

fn reason_to_receive(reason: FailReason) -> ReceiveFailure {
    match reason {
        FailReason::DiskFull => ReceiveFailure::DiskFull,
        FailReason::WriteFailed => ReceiveFailure::WriteFailed,
        FailReason::VerificationFailed => ReceiveFailure::VerificationFailed,
        FailReason::Cancelled => ReceiveFailure::Cancelled,
    }
}

// ---- 云盘 ----

fn kept(id: &str, name: &str, size: u64, sha: &str) -> DriveFile {
    DriveFile {
        id: id.into(),
        name: name.into(),
        size,
        mime_type: None,
        sha256: sha.to_owned(),
        created_at: Timestamp::now(),
    }
}

#[test]
fn a_draft_can_be_sent_to_the_drive_instead_of_a_device() {
    let now = Instant::now();
    let mut registry = Registry::default();
    registry.set_draft(vec![file("合同.pdf", 10, &"a".repeat(64))]);
    let ids = registry.queue_draft(Destination::Drive);
    assert_eq!(ids, ["out-1"]);
    assert!(registry.draft_view().is_none());

    let view = &registry.views()[0];
    assert_eq!(view.direction, Direction::Out);
    assert_eq!(view.target, TransferTarget::Drive);
    assert_eq!(view.peer, "云盘");
    assert_eq!(serde_json::to_value(view).unwrap()["target"], "drive");

    let outgoing = registry.begin_send(&ids[0]).expect("queued");
    assert_eq!(outgoing.destination, Destination::Drive);
    assert_eq!(registry.next_job(now), Next::Nothing);

    // 存进云盘就完事了：没有"等对方接收"，也不欠谁回执。
    assert!(registry.drive_uploaded(&ids[0]));
    let view = view_of(&registry, &ids[0]);
    assert_eq!(view.phase, TransferPhase::Complete);
    assert!(nearly(view.progress, 1.0));
    assert!(!view.retryable);
    assert_eq!(registry.next_job(now), Next::Nothing);
    // 再报一次"存好了"不改变什么。
    assert!(!registry.drive_uploaded(&ids[0]));
}

#[test]
fn a_drive_upload_that_did_not_fit_can_be_tried_again() {
    let now = Instant::now();
    let mut registry = Registry::default();
    registry.set_draft(vec![file("备份.zip", 10, &"a".repeat(64))]);
    let ids = registry.queue_draft(Destination::Drive);
    registry.begin_send(&ids[0]);
    registry.send_failed(&ids[0], TransferFailure::QuotaExceeded);
    let view = view_of(&registry, &ids[0]);
    assert_eq!(view.phase, TransferPhase::Failed);
    assert_eq!(view.failure, Some(TransferFailure::QuotaExceeded));
    assert!(view.retryable);
    // 失败之后不会自己重试：得用户点"重发"。
    assert_eq!(registry.next_job(now), Next::Nothing);
    assert!(registry.retry_failed_send(&ids[0], now));
    assert!(registry.begin_send(&ids[0]).is_some());
}

#[test]
fn a_drive_file_is_queued_for_fetching_and_lands_like_any_other_delivery() {
    let now = Instant::now();
    let mut registry = Registry::default();
    let remote = kept("f1", "合同.pdf", 10, &"a".repeat(64));
    let id = registry.fetch_from_drive(&remote);
    assert_eq!(id, "drive-f1");
    // 同一个文件已经排着：不重复排。
    assert_eq!(registry.fetch_from_drive(&remote), "drive-f1");
    assert_eq!(registry.views().len(), 1);

    let view = &registry.views()[0];
    assert_eq!(view.direction, Direction::In);
    assert_eq!(view.target, TransferTarget::Drive);
    assert_eq!(view.peer, "云盘");
    assert_eq!(view.name, "合同.pdf");
    assert_eq!(view.size, 10);
    assert_eq!(view.phase, TransferPhase::Queued);

    // 取回排在收文件那一队里。
    assert_eq!(
        registry.next_job(now),
        Next::Work(Job::Receive {
            id: "drive-f1".into()
        })
    );
    assert_eq!(
        registry.begin_receive(&id),
        Some(Delivery::FromDrive(remote.clone()))
    );
    // 取回不欠服务端回执。
    registry.receive_completed(&id, "合同.pdf", PathBuf::from("/x/合同.pdf"), 3);
    let view = view_of(&registry, &id);
    assert_eq!(view.phase, TransferPhase::Complete);
    assert_eq!(view.target, TransferTarget::Drive);
    assert_eq!(registry.next_job(now), Next::Nothing);
    assert_eq!(registry.settlement_of(&id), None);
    assert_eq!(registry.saved_file(&id), Some(PathBuf::from("/x/合同.pdf")));
    assert_eq!(
        registry.saved_file_for_handle(3),
        Some(PathBuf::from("/x/合同.pdf"))
    );

    // 取回来之后又排一次：这是一次新的取回。
    assert_eq!(registry.fetch_from_drive(&remote), "drive-f1");
    assert_eq!(registry.views().len(), 1);
    assert_eq!(view_of(&registry, &id).phase, TransferPhase::Queued);
}

#[test]
fn a_drive_file_that_is_gone_is_said_so_and_nothing_is_owed_to_the_server() {
    let now = Instant::now();
    let mut registry = Registry::default();
    let remote = kept("f9", "旧资料.pdf", 10, &"b".repeat(64));
    let id = registry.fetch_from_drive(&remote);
    assert!(registry.begin_receive(&id).is_some());
    registry.receive_failed(&id, TransferFailure::Vanished);
    let view = view_of(&registry, &id);
    assert_eq!(view.phase, TransferPhase::Failed);
    assert_eq!(view.failure, Some(TransferFailure::Vanished));
    assert!(!view.retryable);
    assert_eq!(registry.next_job(now), Next::Nothing);
    assert_eq!(registry.settlement_of(&id), None);
    // 服务端关于"发送"的通知与云盘无关：不改这条记录。
    assert_eq!(
        registry.updated(&settled("f9", SendState::Delivered, None)),
        Updated::Ignored
    );
    assert_eq!(view_of(&registry, &id).phase, TransferPhase::Failed);
}

#[test]
fn a_fetched_drive_file_can_be_cancelled_without_telling_anyone() {
    let mut registry = Registry::default();
    let remote = kept("f1", "合同.pdf", 10, &"a".repeat(64));
    let id = registry.fetch_from_drive(&remote);
    assert_eq!(registry.cancel(&id), Cancellation::Local);
    assert_eq!(view_of(&registry, &id).phase, TransferPhase::Cancelled);
    assert_eq!(registry.next_job(Instant::now()), Next::Nothing);
    assert_eq!(registry.settlement_of(&id), None);
    // 已经排着队的取回被取消之后，还能重新排。
    assert_eq!(registry.fetch_from_drive(&remote), "drive-f1");
    assert_eq!(view_of(&registry, &id).phase, TransferPhase::Queued);
}

#[test]
fn files_queued_without_a_draft_end_up_where_they_are_told() {
    let mut registry = Registry::default();
    let ids = registry.queue(
        Destination::Drive,
        vec![
            file("a.txt", 1, &"a".repeat(64)),
            file("b.txt", 2, &"b".repeat(64)),
        ],
    );
    assert_eq!(ids, ["out-1", "out-2"]);
    assert!(
        registry
            .views()
            .iter()
            .all(|view| view.target == TransferTarget::Drive && view.peer == "云盘")
    );
}
