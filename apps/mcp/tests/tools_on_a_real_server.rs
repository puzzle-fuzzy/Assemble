//! 对着真实运行的服务端，把每个工具走一遍。
//!
//! 这里不关心单个函数，只关心几方拼在一起之后是不是真的能用：工具把助手给的东西整成
//! 协议里的内容、服务端按同一套规则校验、名单回来之后变成给助手看的文字。
//! 登录存在内存里，不碰系统钥匙串。

mod support;

use assemble_client::RequestOptions;
use assemble_mcp::planning::Now;
use assemble_mcp::session::SignedIn;
use assemble_mcp::tools::{
    self, Call, Failure, PlannerInput, ReadInput, ReminderChanges, ReminderInput, ScheduleChanges,
    ScheduleInput, WantedReminderStatus, WantedScheduleStatus,
};
use assemble_protocol::primitives::Timestamp;
use assemble_protocol::schedule::ScheduleStatus;
use assemble_server::http::TestServer;
use tokio_util::sync::CancellationToken;

use support::*;

/// 一个真实的服务端，和一个以助手身份登录的调用环境。
struct World {
    /// 临时目录必须活得比服务久：放掉它，数据库文件就没了。
    _dir: tempfile::TempDir,
    server: TestServer,
    signed_in: SignedIn,
    cancel: CancellationToken,
    now: Now,
}

async fn world() -> World {
    let Fixture {
        _dir: directory,
        server,
    } = fixture().await;
    let device = register(&server, &browser(&server), 'p', ASSISTANT, common::USERNAME).await;
    let me = device.me(&device.options()).await.unwrap();
    World {
        _dir: directory,
        server,
        signed_in: SignedIn {
            client: device.client.clone(),
            account: me.account,
            device: me.device,
        },
        cancel: device.cancel,
        now: Now::at(chrono::Local::now()),
    }
}

impl World {
    fn call(&self) -> Call<'_> {
        Call {
            signed_in: &self.signed_in,
            cancel: &self.cancel,
            now: self.now,
        }
    }

    fn options(&self) -> RequestOptions<'_> {
        options(&self.cancel)
    }

    /// 直接问服务端：名单现在长什么样。
    async fn listing(&self) -> assemble_protocol::schedule::Planner {
        self.signed_in
            .client
            .planner(&self.options())
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn remote_device_tool_lists_only_the_current_accounts_desktops() {
    let mut world = world().await;
    let listing = tools::remote_devices(&world.call()).await.unwrap();
    let devices: Vec<assemble_protocol::device::Device> = serde_json::from_str(&listing).unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].id, world.signed_in.device.id);
    assert!(!listing.contains("token"));
    world.server.stop().await;
}

fn input(value: serde_json::Value) -> ScheduleInput {
    serde_json::from_value(value).unwrap()
}

fn changes(value: serde_json::Value) -> ScheduleChanges {
    serde_json::from_value(value).unwrap()
}

async fn planner(call: &Call<'_>) -> String {
    tools::planner(call, &PlannerInput { days: None })
        .await
        .unwrap()
}
#[tokio::test]
async fn a_schedule_lives_through_create_edit_finish_reopen_and_delete() {
    let world = world().await;
    let call = world.call();

    // 名单是空的，开头说明了现在、账户和这台助手是谁。
    let empty = planner(&call).await;
    assert!(
        empty.contains("账户：Alex Wang（alex）。你在账户里是一台叫“ZCode 的助手”的设备。"),
        "{empty}"
    );
    assert!(empty.contains("日程：\n（没有）"), "{empty}");

    let created = tools::create_schedule(
        &call,
        &input(serde_json::json!({
            "title": " 火车 ", "start": tomorrow_nine(), "remind_before_minutes": 90,
            "location": "上海虹桥站", "notes": "带身份证"
        })),
    )
    .await
    .unwrap();
    assert!(created.starts_with("已保存：火车 [id "), "{created}");
    assert!(created.contains("v1]"), "{created}");
    assert!(
        created.contains("提前 90 分钟提醒 · 备注：带身份证"),
        "{created}"
    );
    let id = id_of(&created);

    // 名单里有了它；记下它的是这台助手。
    let listed = planner(&call).await;
    assert!(listed.contains(&format!("- 火车 [id {id} v1]")), "{listed}");
    assert!(listed.contains("· ZCode 的助手记下的"), "{listed}");

    // 只改开始时间：标题、地点、备注、提前量原样保留。
    let moved = tools::update_schedule(
        &call,
        &id,
        1,
        &changes(serde_json::json!({ "start": tomorrow_at(10, 0) })),
    )
    .await
    .unwrap();
    assert!(
        moved.contains(&format!("已保存：火车 [id {id} v2]")),
        "{moved}"
    );
    assert!(
        moved.contains("上海虹桥站") && moved.contains("带身份证"),
        "{moved}"
    );
    // 去掉结束时间之外的几项：空串就是"去掉"。
    let cleared = tools::update_schedule(
        &call,
        &id,
        2,
        &changes(serde_json::json!({ "location": "", "remind_before_minutes": 0 })),
    )
    .await
    .unwrap();
    assert!(!cleared.contains("上海虹桥站"), "{cleared}");
    assert!(cleared.contains("带身份证"), "{cleared}");
    // 什么都没给：什么都不改。
    let same = tools::update_schedule(&call, &id, 3, &changes(serde_json::json!({})))
        .await
        .unwrap();
    assert!(same.starts_with("什么都没改：火车 [id "), "{same}");

    // 完成、改回进行中、取消。
    let done = tools::set_schedule_status(&call, &id, 3, WantedScheduleStatus::Done)
        .await
        .unwrap();
    assert_eq!(done, "已把日程「火车」标成已完成。");
    // 完成的日程不能再改内容：缘故里说得清楚该怎么办。
    let refused = tools::update_schedule(
        &call,
        &id,
        4,
        &changes(serde_json::json!({ "title": "高铁" })),
    )
    .await
    .unwrap_err();
    assert_eq!(refused, Failure::InvalidState(false));
    tools::set_schedule_status(&call, &id, 4, WantedScheduleStatus::Upcoming)
        .await
        .unwrap();
    let cancelled = tools::set_schedule_status(&call, &id, 5, WantedScheduleStatus::Cancelled)
        .await
        .unwrap();
    assert_eq!(cancelled, "已把日程「火车」标成已取消。");

    // 删掉之后名单里没有它，再动它就说找不到。
    assert_eq!(
        tools::delete_schedule(&call, &id, 6).await.unwrap(),
        "已删除日程「火车」。"
    );
    let gone = tools::update_schedule(
        &call,
        &id,
        7,
        &changes(serde_json::json!({ "title": "高铁" })),
    )
    .await;
    assert_eq!(gone.unwrap_err(), Failure::NotFound);
}

#[tokio::test]
async fn reminders_fire_get_read_and_are_closed() {
    let world = world().await;
    let call = world.call();

    // 一分钟之前就该到点：服务端的调度一轮就会把它点着。
    let due = chrono::Local::now() - chrono::Duration::minutes(1);
    let created = tools::create_reminder(
        &call,
        &ReminderInput {
            title: " 取快递 ".into(),
            due: due.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
            notes: Some("驿站楼下".into()),
        },
    )
    .await
    .unwrap();
    assert!(created.contains("已保存：取快递 [id "), "{created}");
    let id = id_of(&created);

    assemble_server::planner::tick::run_once(&world.server.state, Timestamp::now())
        .await
        .unwrap();
    let listed = planner(&call).await;
    assert!(listed.contains(&format!("[通知 n-{id}]")), "{listed}");
    assert!(
        listed.contains(&format!("- 取快递 [id {id} v2]")),
        "{listed}"
    );
    assert!(listed.contains("· 已到点"), "{listed}");

    // 到点的提醒不能再改。
    let refused = tools::update_reminder(
        &call,
        &id,
        2,
        &ReminderChanges {
            title: None,
            due: Some(tomorrow_nine()),
            notes: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(refused, Failure::InvalidState(true));

    // 读掉通知，再把提醒标成办完了：两样都不再出现。
    let read = tools::read_notifications(
        &call,
        &ReadInput {
            ids: Some(vec![format!("n-{id}")]),
            all: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(read, "已把 1 条到点提醒标成已读。");
    let dismissed = tools::set_reminder_status(&call, &id, 2, WantedReminderStatus::Dismissed)
        .await
        .unwrap();
    assert_eq!(dismissed, "已把提醒「取快递」标成办完了。");
    let listed = planner(&call).await;
    assert!(!listed.contains("取快递"), "{listed}");

    // 没到点的可以改；然后不办了；再删掉。
    let second = tools::create_reminder(
        &call,
        &ReminderInput {
            title: "给妈妈打电话".into(),
            due: tomorrow_nine(),
            notes: None,
        },
    )
    .await
    .unwrap();
    let second_id = id_of(&second);
    let moved = tools::update_reminder(
        &call,
        &second_id,
        1,
        &ReminderChanges {
            title: None,
            due: Some(tomorrow_at(20, 0)),
            notes: Some("带上礼物".into()),
        },
    )
    .await
    .unwrap();
    assert!(
        moved.contains("v2]") && moved.contains("带上礼物"),
        "{moved}"
    );
    let cancelled =
        tools::set_reminder_status(&call, &second_id, 2, WantedReminderStatus::Cancelled)
            .await
            .unwrap();
    assert_eq!(cancelled, "已把提醒「给妈妈打电话」标成不办了。");
    assert_eq!(
        tools::delete_reminder(&call, &second_id, 3).await.unwrap(),
        "已删除提醒「给妈妈打电话」。"
    );
    assert_eq!(
        tools::delete_reminder(&call, &second_id, 4)
            .await
            .unwrap_err(),
        Failure::NotFound
    );

    // 不给 id 也不用 all：说清楚该怎么办。
    assert!(matches!(
        tools::read_notifications(&call, &ReadInput::default()).await,
        Err(Failure::Rejected(_))
    ));
}

#[tokio::test]
async fn an_assistant_sees_and_touches_only_its_own_account() {
    let world = world().await;
    let call = world.call();
    // 另一个人：另一个账户、另一个浏览器、另一台设备。
    create_account(&world.server, "blake", "Blake").await;
    let theirs = register(
        &world.server,
        &browser(&world.server),
        'q',
        "Blake 的电脑",
        "blake",
    )
    .await;
    let saved = theirs
        .create_schedule(&daily("Blake 的会"), None, &theirs.options())
        .await
        .unwrap();

    // 我这边的名单里没有它。
    let listed = planner(&call).await;
    assert!(!listed.contains("Blake"), "{listed}");

    // 也动不了：改、完成、删，都是"找不到"，而不是泄露"它属于别人"。
    let touch = tools::update_schedule(
        &call,
        &saved.id,
        1,
        &changes(serde_json::json!({ "title": "偷改" })),
    )
    .await;
    assert_eq!(touch.unwrap_err(), Failure::NotFound);
    let touch = tools::set_schedule_status(&call, &saved.id, 1, WantedScheduleStatus::Done).await;
    assert_eq!(touch.unwrap_err(), Failure::NotFound);
    let touch = tools::delete_schedule(&call, &saved.id, 1).await;
    assert_eq!(touch.unwrap_err(), Failure::NotFound);

    // 那边原封不动。
    let after = theirs.planner(&theirs.options()).await.unwrap();
    assert_eq!(after.schedules.len(), 1);
    assert_eq!(after.schedules[0].content.title, "Blake 的会");
    assert_eq!(after.schedules[0].status, ScheduleStatus::Upcoming);
}

#[tokio::test]
async fn a_retry_with_the_same_key_creates_one_schedule() {
    let world = world().await;
    // 工具每次新建都带一个新的幂等键；请求超时之后客户端拿同一个键重试，靠的是服务端这一条。
    let first = world
        .signed_in
        .client
        .create_schedule(&daily("周会"), Some("the-same-key"), &world.options())
        .await
        .unwrap();
    let again = world
        .signed_in
        .client
        .create_schedule(&daily("周会"), Some("the-same-key"), &world.options())
        .await
        .unwrap();
    assert_eq!(first.id, again.id);
    assert_eq!(planner(&world.call()).await.matches("周会 [id").count(), 1);

    let reminder = assemble_protocol::schedule::ReminderContent {
        title: "取快递".into(),
        due_at: at(&tomorrow_nine()),
        notes: None,
    };
    let first = world
        .signed_in
        .client
        .create_reminder(&reminder, Some("another-key"), &world.options())
        .await
        .unwrap();
    let again = world
        .signed_in
        .client
        .create_reminder(&reminder, Some("another-key"), &world.options())
        .await
        .unwrap();
    assert_eq!(first.id, again.id);
}

#[tokio::test]
async fn content_the_tools_refuse_never_reaches_the_server() {
    let world = world().await;
    let call = world.call();
    let attempts = [
        // 没带时区。
        serde_json::json!({ "title": "火车", "start": "2026-11-26T09:00:00" }),
        // 没有标题。
        serde_json::json!({ "title": "  ", "start": tomorrow_nine() }),
        // 结束早于开始。
        serde_json::json!({ "title": "火车", "start": tomorrow_nine(), "end": "2000-01-01T00:00:00Z" }),
        // 提前得太多了。
        serde_json::json!({ "title": "火车", "start": tomorrow_nine(), "remind_before_minutes": 2000 }),
    ];
    for attempt in attempts {
        let failure = tools::create_schedule(&call, &input(attempt.clone()))
            .await
            .unwrap_err();
        assert!(
            matches!(failure, Failure::Rejected(_)),
            "{attempt}: {failure:?}"
        );
    }
    // 名单里什么都没多出来。
    assert_eq!(world.listing().await.schedules.len(), 0);
}

#[tokio::test]
async fn a_stale_version_is_reported_with_what_the_schedule_looks_like_now() {
    let world = world().await;
    let call = world.call();
    let created = tools::create_schedule(
        &call,
        &input(serde_json::json!({ "title": "火车", "start": tomorrow_nine() })),
    )
    .await
    .unwrap();
    let id = id_of(&created);
    // 别处（在这里是同一个助手的上一次调用）已经把它改过：版本号涨到 2。
    tools::update_schedule(
        &call,
        &id,
        1,
        &changes(serde_json::json!({ "notes": "带身份证" })),
    )
    .await
    .unwrap();

    // 带着旧版本号来的改动不生效；缘由里带着它现在的样子。
    let stale = tools::set_schedule_status(&call, &id, 1, WantedScheduleStatus::Done)
        .await
        .unwrap_err();
    match stale {
        Failure::ChangedElsewhere(current) => {
            assert!(current.contains(&format!("火车 [id {id} v2]")), "{current}");
            assert!(current.contains("带身份证"), "{current}");
        }
        other => panic!("expected a version conflict, got {other:?}"),
    }
    // 服务端那边仍是进行中、版本号 2。
    let after = world.listing().await;
    assert_eq!(after.schedules[0].status, ScheduleStatus::Upcoming);
    assert_eq!(after.schedules[0].version, 2);
}

#[tokio::test]
async fn overlapping_schedules_are_saved_anyway_with_a_hint() {
    let world = world().await;
    let call = world.call();
    let meeting_start = tomorrow_at(10, 0);
    let meeting_end = tomorrow_at(11, 0);
    tools::create_schedule(
        &call,
        &input(serde_json::json!({ "title": "周会", "start": meeting_start, "end": meeting_end })),
    )
    .await
    .unwrap();

    // 十点半的牙医和它撞上：照样保存，结果里点名。
    let dentist = tools::create_schedule(
        &call,
        &input(serde_json::json!({ "title": "看牙", "start": tomorrow_at(10, 30) })),
    )
    .await
    .unwrap();
    assert!(dentist.starts_with("已保存：看牙 [id "), "{dentist}");
    assert!(dentist.contains("提示：它和「周会」"), "{dentist}");

    // 十一点的火车紧跟着周会：不算撞车，没有提示。
    let train = tools::create_schedule(
        &call,
        &input(serde_json::json!({ "title": "火车", "start": meeting_end })),
    )
    .await
    .unwrap();
    assert!(!train.contains("提示"), "{train}");
    assert_eq!(world.listing().await.schedules.len(), 3);
}

#[tokio::test]
async fn private_memo_tools_preserve_text_and_use_versions_and_retry_keys() {
    let world = world().await;
    let call = world.call();
    let input = tools::MemoCreateInput {
        request_id: "memo-tool-request".into(),
        title: "常用文字".into(),
        text: " 空格\n\t纯文本🙂\n".into(),
    };
    let result = tools::memo_create(&call, &input).await.unwrap();
    let memo: assemble_protocol::memo::Memo = serde_json::from_str(&result).unwrap();
    assert_eq!(memo.content.text, input.text);
    assert_eq!(tools::memo_create(&call, &input).await.unwrap(), result);
    let listing = tools::memos(
        &call,
        &tools::MemoListInput {
            q: "纯文本".into(),
            page: 1,
        },
    )
    .await
    .unwrap();
    assert!(listing.contains(&memo.id));
    assert_eq!(tools::memo_get(&call, &memo.id).await.unwrap(), result);
    let update = tools::MemoUpdateInput {
        id: memo.id.clone(),
        version: 1,
        title: "新标题".into(),
        text: "新内容".into(),
    };
    tools::memo_update(&call, &update).await.unwrap();
    let failed = tools::memo_update(&call, &update)
        .await
        .unwrap_err()
        .message();
    assert!(failed.contains("assemble_memo_get"));
    tools::memo_remove(&call, &memo.id, 2).await.unwrap();
    assert!(
        tools::memo_get(&call, &memo.id)
            .await
            .unwrap_err()
            .message()
            .contains("备忘录")
    );
}
