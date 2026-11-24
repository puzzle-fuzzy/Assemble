//! 日程规划的集成测试：对真实运行的服务走完整条链路。
//!
//! 这里核对的是几方拼起来的行为：版本号怎么拦住互相覆盖、幂等键怎么挡住重复新建、
//! 通知什么时候作废、每一处改动怎么推到账户里的所有设备。
//! 到点那一刻由调度负责，它的测试在 `apps/server/src/planner/tick.rs`；
//! 这里需要一条已经生成的通知时，直接往库里写。

mod common;

use assemble_client::{Client, EventStream, ReminderAction, ScheduleAction};
use assemble_protocol::events::Event;
use assemble_protocol::primitives::{Platform, Timestamp};
use assemble_protocol::schedule::{
    NotificationKind, Planner, ReminderContent, ReminderStatus, Repeat, Schedule, ScheduleContent,
    ScheduleStatus, ScheduleUpdate,
};
use assemble_store::secret::SecretString;
use common::*;
use sqlx::SqlitePool;

// ---- 工具 ----

fn at(text: &str) -> Timestamp {
    serde_json::from_value(serde_json::json!(text)).unwrap()
}

fn train() -> ScheduleContent {
    ScheduleContent {
        title: " 火车 ".into(),
        location: None,
        start_at: at("2026-11-25T01:00:00.000Z"),
        end_at: None,
        repeat: Repeat::None,
        repeat_until: None,
        remind_before: None,
        notes: None,
    }
}

fn daily() -> ScheduleContent {
    ScheduleContent {
        repeat: Repeat::Daily,
        ..train()
    }
}

fn reminder(title: &str, due: &str) -> ReminderContent {
    ReminderContent {
        title: title.into(),
        due_at: at(due),
        notes: None,
    }
}

fn http(status: u16, code: &str) -> assemble_client::ClientError {
    assemble_client::ClientError::Http {
        status,
        remote_code: Some(code.to_owned()),
    }
}

async fn planner_of(pc: &Registered) -> Planner {
    pc.planner(&pc.options()).await.unwrap()
}

/// 一直读到下一条面板事件，把它带的名单交出来。中间的问候与设备变化事件跳过。
async fn next_planner(stream: &mut EventStream) -> Planner {
    loop {
        if let Event::PlannerChanged { planner } = next_event(stream).await {
            return planner;
        }
    }
}

/// 直接往库里写一条通知，冒充调度生成的那一条。
async fn raised(
    pool: &SqlitePool,
    account_id: &str,
    id: &str,
    schedule_id: Option<&str>,
    occurrence: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO notifications \
         (id, account_id, kind, title, body, schedule_id, reminder_id, occurrence_at, raised_at) \
         VALUES (?, ?, 'schedule', '火车', NULL, ?, NULL, ?, ?)",
    )
    .bind(id)
    .bind(account_id)
    .bind(schedule_id)
    .bind(occurrence)
    // 名单只收最近七天的：冒充刚生成的。
    .bind(Timestamp::now().as_millis())
    .execute(pool)
    .await
    .unwrap();
}

/// 一条通知的已读时间与作废时间。
async fn marks(pool: &SqlitePool, id: &str) -> (Option<i64>, Option<i64>) {
    sqlx::query_as::<_, (Option<i64>, Option<i64>)>(
        "SELECT read_at, invalidated_at FROM notifications WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// 不经客户端的校验，直接发请求：用来确认服务端自己也拦得住。
fn raw(
    pc: &Registered,
    token: &SecretString,
    method: reqwest::Method,
    path: &str,
) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .request(method, pc.client.origin().join(path))
        .bearer_auth(token.expose())
}

/// 另一个账户的一台设备，连同那个账户的标识。登记要写数据库，只在这里的测试环境里用。
async fn stranger(fixture: &Fixture) -> (Registered, String) {
    let pool = &fixture.server.state.pool;
    let account = match assemble_server::accounts::find_by_username(pool, "stranger")
        .await
        .unwrap()
    {
        Some(account) => account,
        None => assemble_server::accounts::create(
            pool,
            "stranger",
            "别人",
            SecretString::new("another-long-enough-password".to_owned()),
        )
        .await
        .unwrap(),
    };
    let token = assemble_server::secrets::random_token().unwrap();
    let device = assemble_server::devices::register(
        pool,
        &account.id,
        &assemble_protocol::device::NewDevice {
            name: "别的账户的电脑".into(),
            platform: Platform::Macos,
        },
        &assemble_server::secrets::token_digest(token.expose()),
    )
    .await
    .unwrap();
    let client = Client::new(fixture.server.origin.clone()).unwrap();
    client.set_bearer(Some(token.clone()));
    let registered = Registered {
        client,
        token,
        device: assemble_protocol::device::Device {
            id: device.id,
            name: device.name,
            platform: Platform::Macos,
            online: false,
            last_seen_at: None,
            created_at: Timestamp::now(),
        },
        cancel: tokio_util::sync::CancellationToken::new(),
    };
    (registered, account.id)
}

// ---- 日程 ----

#[tokio::test]
async fn a_schedule_lives_through_create_edit_finish_and_reopen() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'p', "本机", Platform::Macos).await;

    let created = pc
        .create_schedule(&train(), None, &pc.options())
        .await
        .unwrap();
    assert_eq!(created.version, 1);
    assert_eq!(created.status, ScheduleStatus::Upcoming);
    // 两端的空白去掉了；记着是谁建的。
    assert_eq!(created.content.title, "火车");
    assert_eq!(created.created_by, "本机");
    let planner = planner_of(&pc).await;
    assert_eq!(planner.schedules, vec![created.clone()]);
    assert_eq!(planner.notifications, Vec::new());

    let moved = ScheduleUpdate {
        version: 1,
        content: ScheduleContent {
            title: "火车（改点）".into(),
            start_at: at("2026-11-25T02:00:00.000Z"),
            notes: Some(" 带身份证 ".into()),
            ..train()
        },
    };
    let updated = pc
        .update_schedule(&created.id, &moved, &pc.options())
        .await
        .unwrap();
    assert_eq!(updated.version, 2);
    assert_eq!(updated.content.title, "火车（改点）");
    assert_eq!(updated.content.notes.as_deref(), Some("带身份证"));
    assert_eq!(updated.content.start_at, at("2026-11-25T02:00:00.000Z"));
    assert_eq!(updated.created_at, created.created_at);

    let done = pc
        .set_schedule_status(&created.id, ScheduleAction::Complete, 2, &pc.options())
        .await
        .unwrap();
    assert_eq!(done.status, ScheduleStatus::Done);
    assert_eq!(done.version, 3);
    // 刚完成的还留在名单里：界面上还能看一眼、还能撤回。
    assert_eq!(planner_of(&pc).await.schedules, vec![done.clone()]);
    // 已经完成的不能再改内容。
    let again = ScheduleUpdate {
        version: 3,
        ..moved
    };
    assert_eq!(
        pc.update_schedule(&created.id, &again, &pc.options())
            .await
            .unwrap_err(),
        http(409, "INVALID_STATE")
    );

    let reopened = pc
        .set_schedule_status(&created.id, ScheduleAction::Reopen, 3, &pc.options())
        .await
        .unwrap();
    assert_eq!(reopened.status, ScheduleStatus::Upcoming);
    assert_eq!(reopened.version, 4);
    // 重复一次同样的状态：什么都不变，连版本号也不涨。
    let same = pc
        .set_schedule_status(&created.id, ScheduleAction::Reopen, 4, &pc.options())
        .await
        .unwrap();
    assert_eq!(same, reopened);
}

#[tokio::test]
async fn a_stale_version_is_rejected_instead_of_overwriting_someone_elses_change() {
    let fixture = fixture().await;
    let laptop = register(&fixture, 'l', "笔记本", Platform::Macos).await;
    let phone = register(&fixture, 'h', "手机", Platform::Windows).await;

    let created = laptop
        .create_schedule(&train(), None, &laptop.options())
        .await
        .unwrap();
    // 另一台设备先改了。
    let theirs = ScheduleUpdate {
        version: 1,
        content: ScheduleContent {
            title: "别人改过的".into(),
            ..train()
        },
    };
    phone
        .update_schedule(&created.id, &theirs, &phone.options())
        .await
        .unwrap();

    // 这一台还拿着旧的版本号：改、完成、删，都被挡住，谁也没覆盖谁。
    let mine = update(
        1,
        ScheduleContent {
            title: "我的改动".into(),
            ..train()
        },
    );
    for error in [
        laptop
            .update_schedule(&created.id, &mine, &laptop.options())
            .await
            .unwrap_err(),
        laptop
            .set_schedule_status(&created.id, ScheduleAction::Complete, 1, &laptop.options())
            .await
            .unwrap_err(),
        laptop
            .set_schedule_status(&created.id, ScheduleAction::Cancel, 1, &laptop.options())
            .await
            .unwrap_err(),
        laptop
            .delete_schedule(&created.id, 1, &laptop.options())
            .await
            .unwrap_err(),
    ] {
        assert_eq!(error, http(409, "VERSION_CONFLICT"));
    }
    let current = planner_of(&laptop).await.schedules.remove(0);
    assert_eq!(current.content.title, "别人改过的");
    assert_eq!(current.status, ScheduleStatus::Upcoming);
}

fn update(version: i64, content: ScheduleContent) -> ScheduleUpdate {
    ScheduleUpdate { version, content }
}

#[tokio::test]
async fn an_idempotency_key_creates_only_one_schedule() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'p', "本机", Platform::Macos).await;

    // 同一个键发两遍（比如第一次的回执丢了）：拿回的是同一条。
    let first = pc
        .create_schedule(&train(), Some("call-1"), &pc.options())
        .await
        .unwrap();
    let again = pc
        .create_schedule(
            &ScheduleContent {
                title: "内容都不一样了".into(),
                ..train()
            },
            Some("call-1"),
            &pc.options(),
        )
        .await
        .unwrap();
    assert_eq!(again, first);
    assert_eq!(planner_of(&pc).await.schedules.len(), 1);

    // 不带键就是两条；键不同也是两条。
    pc.create_schedule(&train(), None, &pc.options())
        .await
        .unwrap();
    let other = pc
        .create_schedule(&train(), Some("call-2"), &pc.options())
        .await
        .unwrap();
    assert_ne!(other.id, first.id);
    assert_eq!(planner_of(&pc).await.schedules.len(), 3);

    // 键的写法有讲究：不合规矩的请求被拒绝，什么都没建。
    assert_eq!(
        pc.create_schedule(&train(), Some("有 空格"), &pc.options())
            .await
            .unwrap_err(),
        http(400, "INVALID_REQUEST")
    );
    assert_eq!(planner_of(&pc).await.schedules.len(), 3);
}

#[tokio::test]
async fn the_server_itself_rejects_content_that_breaks_the_rules() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'p', "本机", Platform::Macos).await;

    for broken in [
        serde_json::json!({ "title": "   ", "startAt": "2026-11-25T01:00:00.000Z" }),
        serde_json::json!({ "title": "火车", "startAt": "2026-11-25T02:00:00.000Z", "endAt": "2026-11-25T01:00:00.000Z" }),
        serde_json::json!({ "title": "火车", "startAt": "1899-01-01T00:00:00.000Z" }),
        serde_json::json!({ "title": "火车", "startAt": "不是时间" }),
    ] {
        let response = raw(&pc, &pc.token, reqwest::Method::POST, "/v1/schedules")
            .json(&broken)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "{broken}"
        );
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], "INVALID_REQUEST");
    }
    assert_eq!(planner_of(&pc).await.schedules, Vec::new());
}

#[tokio::test]
async fn cancelling_or_finishing_withdraws_the_reminders_that_were_not_read() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'p', "本机", Platform::Macos).await;
    let pool = &fixture.server.state.pool;
    let account = account_of(&fixture).await;

    let cancelled = pc
        .create_schedule(&train(), None, &pc.options())
        .await
        .unwrap();
    let first = format!(
        "{}-{}",
        cancelled.id,
        cancelled.content.start_at.as_millis()
    );
    raised(
        pool,
        &account,
        &first,
        Some(&cancelled.id),
        Some(cancelled.content.start_at.as_millis()),
    )
    .await;
    assert_eq!(planner_of(&pc).await.notifications.len(), 1);

    let gone = pc
        .set_schedule_status(&cancelled.id, ScheduleAction::Cancel, 1, &pc.options())
        .await
        .unwrap();
    assert_eq!(gone.status, ScheduleStatus::Cancelled);
    assert_eq!(planner_of(&pc).await.notifications, Vec::new());
    // 记录还在，只是作废了：删掉的话，同一场又会被当成没提醒过。
    let (read, invalidated) = marks(pool, &first).await;
    assert_eq!((read, invalidated.is_some()), (None, true));

    let finished = pc
        .create_schedule(&train(), None, &pc.options())
        .await
        .unwrap();
    let second = format!("{}-{}", finished.id, 1);
    raised(pool, &account, &second, Some(&finished.id), Some(1)).await;
    pc.set_schedule_status(&finished.id, ScheduleAction::Complete, 1, &pc.options())
        .await
        .unwrap();
    assert!(marks(pool, &second).await.1.is_some());
}

#[tokio::test]
async fn rescheduling_only_withdraws_the_occurrences_that_no_longer_exist() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'p', "本机", Platform::Macos).await;
    let pool = &fixture.server.state.pool;
    let account = account_of(&fixture).await;
    let day = 24 * 60 * 60 * 1000;

    let created = pc
        .create_schedule(&daily(), None, &pc.options())
        .await
        .unwrap();
    let first_day = created.content.start_at.as_millis();
    let second_day = first_day + day;
    let first = format!("{}-{first_day}", created.id);
    let second = format!("{}-{second_day}", created.id);
    raised(pool, &account, &first, Some(&created.id), Some(first_day)).await;
    raised(pool, &account, &second, Some(&created.id), Some(second_day)).await;

    // 只改标题：两条提醒都还在，标题跟着换了。
    let renamed = ScheduleUpdate {
        version: 1,
        content: ScheduleContent {
            title: "周会".into(),
            ..daily()
        },
    };
    pc.update_schedule(&created.id, &renamed, &pc.options())
        .await
        .unwrap();
    let planner = planner_of(&pc).await;
    assert_eq!(planner.notifications.len(), 2);
    assert!(planner.notifications.iter().all(|n| n.title == "周会"));

    // 重复到第一场之后就停：第二场不再成立，它的提醒作废；第一场的留着。
    let shortened = ScheduleUpdate {
        version: 2,
        content: ScheduleContent {
            repeat_until: Some(Timestamp::from_millis(first_day + 1000).unwrap()),
            ..daily()
        },
    };
    pc.update_schedule(&created.id, &shortened, &pc.options())
        .await
        .unwrap();
    let planner = planner_of(&pc).await;
    assert_eq!(
        planner
            .notifications
            .iter()
            .map(|n| n.id.as_str())
            .collect::<Vec<_>>(),
        vec![first.as_str()]
    );
    assert!(marks(pool, &second).await.1.is_some());
    assert_eq!(marks(pool, &first).await.1, None);

    // 整体挪后一小时：原来那两场都不再是任何一场，全作废。
    let shifted = ScheduleUpdate {
        version: 3,
        content: ScheduleContent {
            start_at: at("2026-11-25T02:00:00.000Z"),
            ..daily()
        },
    };
    pc.update_schedule(&created.id, &shifted, &pc.options())
        .await
        .unwrap();
    assert_eq!(planner_of(&pc).await.notifications, Vec::new());
}

#[tokio::test]
async fn deleting_a_schedule_takes_its_notifications_along() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'p', "本机", Platform::Macos).await;
    let pool = &fixture.server.state.pool;
    let account = account_of(&fixture).await;

    let created = pc
        .create_schedule(&train(), None, &pc.options())
        .await
        .unwrap();
    let note = format!("{}-1", created.id);
    raised(pool, &account, &note, Some(&created.id), Some(1)).await;
    assert_eq!(
        pc.delete_schedule(&created.id, 7, &pc.options())
            .await
            .unwrap_err(),
        http(409, "VERSION_CONFLICT")
    );
    pc.delete_schedule(&created.id, 1, &pc.options())
        .await
        .unwrap();
    assert_eq!(planner_of(&pc).await.schedules, Vec::new());
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications WHERE schedule_id = ?")
        .bind(&created.id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    // 删掉之后当它不存在：别的账户与本账户的回答一样。
    let gone = update(
        1,
        ScheduleContent {
            title: "改动".into(),
            ..train()
        },
    );
    assert_eq!(
        pc.update_schedule(&created.id, &gone, &pc.options())
            .await
            .unwrap_err(),
        http(404, "NOT_FOUND")
    );
}

#[tokio::test]
async fn another_account_cannot_see_or_touch_a_schedule() {
    let fixture = fixture().await;
    let owner = register(&fixture, 'o', "本机", Platform::Macos).await;
    let (stranger, _) = stranger(&fixture).await;

    let created = owner
        .create_schedule(&train(), None, &owner.options())
        .await
        .unwrap();
    assert_eq!(planner_of(&stranger).await.schedules, Vec::new());
    let change = update(
        1,
        ScheduleContent {
            title: "改动".into(),
            ..train()
        },
    );
    assert_eq!(
        stranger
            .update_schedule(&created.id, &change, &stranger.options())
            .await
            .unwrap_err(),
        http(404, "NOT_FOUND")
    );
    // 幂等键按账户分开：另一个账户用同一个键，建出自己的那一条。
    let theirs = stranger
        .create_reminder(
            &reminder("别人的", "2026-11-25T01:00:00.000Z"),
            Some("k"),
            &stranger.options(),
        )
        .await
        .unwrap();
    let mine = owner
        .create_reminder(
            &reminder("我的", "2026-11-25T01:00:00.000Z"),
            Some("k"),
            &owner.options(),
        )
        .await
        .unwrap();
    assert_ne!(theirs.id, mine.id);
}

// ---- 提醒 ----

#[tokio::test]
async fn a_reminder_can_be_edited_until_it_fires() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'p', "本机", Platform::Macos).await;
    let pool = &fixture.server.state.pool;

    let created = pc
        .create_reminder(
            &reminder(" 取快递 ", "2026-11-25T01:00:00.000Z"),
            None,
            &pc.options(),
        )
        .await
        .unwrap();
    assert_eq!(created.status, ReminderStatus::Scheduled);
    assert_eq!(created.content.title, "取快递");
    assert_eq!(planner_of(&pc).await.reminders, vec![created.clone()]);

    let later = assemble_protocol::schedule::ReminderUpdate {
        version: 1,
        content: reminder("取快递", "2026-11-25T03:00:00.000Z"),
    };
    let updated = pc
        .update_reminder(&created.id, &later, &pc.options())
        .await
        .unwrap();
    assert_eq!(updated.version, 2);
    assert_eq!(updated.content.due_at, at("2026-11-25T03:00:00.000Z"));

    // 到点之后（这里直接把状态改成 fired，冒充调度干的）：不能再改时间。
    sqlx::query("UPDATE reminders SET status = 'fired' WHERE id = ?")
        .bind(&created.id)
        .execute(pool)
        .await
        .unwrap();
    let stale = assemble_protocol::schedule::ReminderUpdate {
        version: 2,
        ..later
    };
    assert_eq!(
        pc.update_reminder(&created.id, &stale, &pc.options())
            .await
            .unwrap_err(),
        http(409, "INVALID_STATE")
    );
    let dismissed = pc
        .set_reminder_status(&created.id, ReminderAction::Dismiss, 2, &pc.options())
        .await
        .unwrap();
    assert_eq!(dismissed.status, ReminderStatus::Dismissed);
    // 办完的提醒不再能取消。
    assert_eq!(
        pc.set_reminder_status(&created.id, ReminderAction::Cancel, 3, &pc.options())
            .await
            .unwrap_err(),
        http(409, "INVALID_STATE")
    );
}

#[tokio::test]
async fn cancelling_a_reminder_withdraws_its_notification() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'p', "本机", Platform::Macos).await;
    let pool = &fixture.server.state.pool;
    let account = account_of(&fixture).await;

    let created = pc
        .create_reminder(
            &reminder("取快递", "2026-11-25T01:00:00.000Z"),
            None,
            &pc.options(),
        )
        .await
        .unwrap();
    let note = format!("n-{}", created.id);
    sqlx::query(
        "INSERT INTO notifications \
         (id, account_id, kind, title, schedule_id, reminder_id, raised_at) \
         VALUES (?, ?, 'reminder', '取快递', NULL, ?, ?)",
    )
    .bind(&note)
    .bind(&account)
    .bind(&created.id)
    .bind(Timestamp::now().as_millis())
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(planner_of(&pc).await.notifications.len(), 1);

    let cancelled = pc
        .set_reminder_status(&created.id, ReminderAction::Cancel, 1, &pc.options())
        .await
        .unwrap();
    assert_eq!(cancelled.status, ReminderStatus::Cancelled);
    assert_eq!(planner_of(&pc).await.notifications, Vec::new());
    pc.delete_reminder(&created.id, 2, &pc.options())
        .await
        .unwrap();
    assert_eq!(planner_of(&pc).await.reminders, Vec::new());
}

// ---- 通知 ----

#[tokio::test]
async fn notifications_can_be_read_one_by_one_or_all_at_once() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'p', "本机", Platform::Macos).await;
    let pool = &fixture.server.state.pool;
    let account = account_of(&fixture).await;
    let (stranger, stranger_account) = stranger(&fixture).await;

    for id in ["note-1", "note-2"] {
        raised(pool, &account, id, None, None).await;
    }
    raised(pool, &stranger_account, "theirs-1", None, None).await;
    assert_eq!(planner_of(&pc).await.notifications.len(), 2);

    pc.mark_notifications_read(&["note-1"], &pc.options())
        .await
        .unwrap();
    let left: Vec<String> = planner_of(&pc)
        .await
        .notifications
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(left, vec!["note-2"]);
    assert!(marks(pool, "note-1").await.0.is_some());

    pc.mark_all_notifications_read(&pc.options()).await.unwrap();
    assert_eq!(planner_of(&pc).await.notifications, Vec::new());
    // 别的账户的不受影响。
    assert_eq!(planner_of(&stranger).await.notifications.len(), 1);

    // 标识不合规矩、或者一次太多：整个请求被拒绝，什么都不改。
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE read_at IS NOT NULL")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        pc.mark_notifications_read(&["a-b'"], &pc.options())
            .await
            .unwrap_err(),
        assemble_client::ClientError::InvalidRequest
    );
    let too_many: Vec<String> = (0..201).map(|n| format!("n{n}")).collect();
    let refs: Vec<&str> = too_many.iter().map(String::as_str).collect();
    assert_eq!(
        pc.mark_notifications_read(&refs, &pc.options())
            .await
            .unwrap_err(),
        http(400, "INVALID_REQUEST")
    );
    let after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE read_at IS NOT NULL")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(before, after);
}

// ---- 推送与名单 ----

#[tokio::test]
async fn every_change_reaches_every_device_of_the_account() {
    let fixture = fixture().await;
    let laptop = register(&fixture, 'l', "笔记本", Platform::Macos).await;
    let phone = register(&fixture, 'h', "手机", Platform::Windows).await;
    let mut laptop_events = laptop.open_events();
    let mut phone_events = phone.open_events();

    let created = laptop
        .create_schedule(&train(), None, &laptop.options())
        .await
        .unwrap();
    // 两台设备都收到整份名单，里面是刚建的那一条。
    assert_eq!(
        next_planner(&mut laptop_events).await.schedules,
        vec![created.clone()]
    );
    assert_eq!(
        next_planner(&mut phone_events).await.schedules,
        vec![created.clone()]
    );

    // 一台设备标成完成，另一台立刻看到。
    let done = phone
        .set_schedule_status(&created.id, ScheduleAction::Complete, 1, &phone.options())
        .await
        .unwrap();
    assert_eq!(
        next_planner(&mut laptop_events).await.schedules,
        vec![done.clone()]
    );
}

#[tokio::test]
async fn the_listing_keeps_recently_finished_items_but_forgets_old_ones() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'p', "本机", Platform::Macos).await;
    let pool = &fixture.server.state.pool;
    let account = account_of(&fixture).await;
    let week = 7 * 24 * 60 * 60 * 1000;

    let created = pc
        .create_schedule(&train(), None, &pc.options())
        .await
        .unwrap();
    let done = pc
        .set_schedule_status(&created.id, ScheduleAction::Complete, 1, &pc.options())
        .await
        .unwrap();
    assert_eq!(planner_of(&pc).await.schedules, vec![done.clone()]);

    // 八天前就完成的：不再出现在名单里，但记录还在。
    sqlx::query("UPDATE schedules SET updated_at = ? WHERE id = ?")
        .bind(Timestamp::now().as_millis() - week - 1000)
        .bind(&created.id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(planner_of(&pc).await.schedules, Vec::new());
    let kept: Option<Schedule> = sqlx::query_as::<
        _,
        assemble_server::planner::schedules::ScheduleRow,
    >("SELECT * FROM schedules WHERE id = ?")
    .bind(&created.id)
    .fetch_optional(pool)
    .await
    .unwrap()
    .map(|row| row.view());
    assert_eq!(kept.unwrap().status, ScheduleStatus::Done);

    // 一周以上还没读的通知也不再补发；一周之内的照常给。
    let now = Timestamp::now().as_millis();
    raised(pool, &account, "fresh-1", None, None).await;
    sqlx::query("UPDATE notifications SET raised_at = ? WHERE id = 'fresh-1'")
        .bind(now - week + 60_000)
        .execute(pool)
        .await
        .unwrap();
    raised(pool, &account, "old-1", None, None).await;
    sqlx::query("UPDATE notifications SET raised_at = ? WHERE id = 'old-1'")
        .bind(now - week - 60_000)
        .execute(pool)
        .await
        .unwrap();
    let ids: Vec<String> = planner_of(&pc)
        .await
        .notifications
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(ids, vec!["fresh-1"]);
}

#[tokio::test]
async fn a_notification_record_reads_back_with_every_field() {
    let fixture = fixture().await;
    let pc = register(&fixture, 'p', "本机", Platform::Macos).await;
    let pool = &fixture.server.state.pool;
    let account = account_of(&fixture).await;

    let created = pc
        .create_schedule(&train(), None, &pc.options())
        .await
        .unwrap();
    let at_ms = created.content.start_at.as_millis();
    raised(
        pool,
        &account,
        &format!("{}-{at_ms}", created.id),
        Some(&created.id),
        Some(at_ms),
    )
    .await;
    let notification = planner_of(&pc).await.notifications.remove(0);
    assert_eq!(notification.kind, NotificationKind::Schedule);
    assert_eq!(
        notification.schedule_id.as_deref(),
        Some(created.id.as_str())
    );
    assert_eq!(notification.occurrence_at, Some(created.content.start_at));
}

#[tokio::test]
async fn old_notifications_are_pruned_but_fresh_ones_stay() {
    let fixture = fixture().await;
    let pool = &fixture.server.state.pool;
    let account = account_of(&fixture).await;
    let month = assemble_server::maintenance::NOTIFICATION_HISTORY_MILLIS;

    for id in ["fresh-1", "fresh-2"] {
        raised(pool, &account, id, None, None).await;
    }
    sqlx::query("UPDATE notifications SET raised_at = ?, read_at = 1 WHERE id = 'fresh-2'")
        .bind(Timestamp::now().as_millis() - month - 1000)
        .execute(pool)
        .await
        .unwrap();

    let report = assemble_server::maintenance::run_once(&fixture.server.state)
        .await
        .unwrap();
    assert_eq!(report.pruned_notifications, 1);
    let left: Vec<String> = sqlx::query_scalar("SELECT id FROM notifications")
        .fetch_all(pool)
        .await
        .unwrap();
    assert_eq!(left, vec!["fresh-1"]);
}
