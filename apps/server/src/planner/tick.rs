//! 到点检查：每隔一会儿看一眼，哪条日程该提醒了、哪条提醒到点了。
//!
//! 时钟由调用方传入：正式运行传现在，测试拨表。
//!
//! - **每一场只通知一次。** 日程的通知编号由"日程 + 场次时刻"决定，数据库不会让同一个
//!   编号出现两行。检查跑重了、两轮挤在同一刻，也都只有一条。
//! - **太晚的不补。** 离开该提醒的时刻超过十分钟，或者这一场已经结束，就不再提醒：
//!   早上九点的会，下午打开电脑不该再弹一遍。服务端停机期间错过的场次同理。
//! - **提醒不管多晚都补。** 它是一件要做的事，晚到也比不到好。
//! - **通知先落库，再推送。** 事件流是"发了就忘"的；设备重新连上时拉名单，
//!   看到的就是库里的那一条。

use std::time::Duration;

use assemble_protocol::primitives::Timestamp;
use assemble_schedule::occurrences::Recurrence;
use tokio_util::sync::CancellationToken;

use super::notifications::schedule_body;
use super::publish;
use super::reminders::ReminderRow;
use super::schedules::ScheduleRow;
use crate::http::Shared;

/// 多久检查一次。十秒的误差对一条提醒来说足够准，平时只是一次很小的查询。
pub const INTERVAL: Duration = Duration::from_secs(10);

/// 离开该提醒的时刻多久之后就不再提醒：十分钟。
pub const GRACE_MILLIS: i64 = 10 * 60 * 1000;

/// 一分钟的毫秒数。
const MINUTE: i64 = 60 * 1000;

/// 一轮检查的结果，只进日志。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// 这一刻到点、被标成已到点的提醒数。
    pub reminders_fired: u32,
    /// 生成的日程提醒数。
    pub schedule_notifications: u32,
}

/// 在后台按固定间隔运行，直到服务端退出。
pub fn start(state: Shared, stop: CancellationToken) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = stop.cancelled() => return,
                _ = ticker.tick() => {}
            }
            match run_once(&state, Timestamp::now()).await {
                Ok(report) if report != Report::default() => {
                    tracing::info!(?report, "due reminders were raised");
                }
                Ok(_) => {}
                Err(error) => tracing::error!(%error, "a due-reminder round failed"),
            }
        }
    });
}

/// 跑一轮：把 `now` 这一刻该提醒的都提醒掉，然后给动过的账户各推一份名单。
pub async fn run_once(state: &Shared, now: Timestamp) -> Result<Report, sqlx::Error> {
    let mut report = Report::default();
    let mut touched: Vec<String> = Vec::new();

    // ---- 提醒：到点就发，不管晚了多久 ----
    let due: Vec<ReminderRow> = sqlx::query_as(
        "SELECT * FROM reminders WHERE status = 'scheduled' AND due_at <= ? ORDER BY due_at, rowid",
    )
    .bind(now.as_millis())
    .fetch_all(&state.pool)
    .await?;
    for reminder in due {
        if fire_reminder(state, &reminder, now).await? {
            report.reminders_fired += 1;
            touched.push(reminder.account_id);
        }
    }

    // ---- 日程：这一刻落在"某一场的提醒时刻"之后十分钟之内 ----
    let max_lead = i64::from(assemble_protocol::schedule::REMIND_BEFORE_MAX_MINUTES) * MINUTE;
    // 先按最早可能的提醒时刻筛一遍：开始时间在"现在 + 最大提前量"之后的，这一轮一定轮不到。
    // 上一次出现最晚也晚不过截止时刻；再往前让出宽限与最大提前量。
    let candidates: Vec<ScheduleRow> = sqlx::query_as(
        "SELECT * FROM schedules \
         WHERE status = 'upcoming' AND start_at <= ? \
         AND (repeat_until IS NULL OR repeat_until >= ?) \
         ORDER BY start_at, rowid",
    )
    .bind(now.as_millis() + max_lead)
    .bind(now.as_millis() - GRACE_MILLIS - max_lead)
    .fetch_all(&state.pool)
    .await?;
    for schedule in candidates {
        if announce_occurrence(state, &schedule, now).await? {
            report.schedule_notifications += 1;
            touched.push(schedule.account_id);
        }
    }

    // ---- 推送：每个动过的账户发一份整名单 ----
    touched.sort();
    touched.dedup();
    for account_id in touched {
        publish(&state.hub, &state.pool, &account_id).await;
    }
    Ok(report)
}

/// 把一条到点的提醒标成已到点，并生成它的通知。已经被别的一轮处理过的返回 `false`。
async fn fire_reminder(
    state: &Shared,
    reminder: &ReminderRow,
    now: Timestamp,
) -> Result<bool, sqlx::Error> {
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    // 带着状态去改：查询与处理之间要是被别的一轮抢先了，这里一行都改不动。
    let changed = sqlx::query(
        "UPDATE reminders SET status = 'fired', version = version + 1, updated_at = ? \
         WHERE id = ? AND status = 'scheduled'",
    )
    .bind(now.as_millis())
    .bind(&reminder.id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 0 {
        tx.rollback().await?;
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO notifications \
         (id, account_id, kind, title, body, schedule_id, reminder_id, occurrence_at, raised_at) \
         VALUES (?, ?, 'reminder', ?, ?, NULL, ?, NULL, ?)",
    )
    .bind(format!("n-{}", reminder.id))
    .bind(&reminder.account_id)
    .bind(&reminder.title)
    .bind(&reminder.notes)
    .bind(&reminder.id)
    .bind(now.as_millis())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// 日程的某一场该提醒了，生成它的通知。
///
/// 候选的是"不晚于 `现在 + 提前量` 的最近一场"，它的提醒时刻是"场次开始减提前量"。
/// 返回 `false` 的情形：没有这样的场次、提醒时刻已经过去太久、这一场已经结束，
/// 或者早就提醒过了。
async fn announce_occurrence(
    state: &Shared,
    schedule: &ScheduleRow,
    now: Timestamp,
) -> Result<bool, sqlx::Error> {
    let lead = schedule.remind_before.unwrap_or(0) * MINUTE;
    let rule = Recurrence::new(
        schedule.start_at,
        schedule.content().repeat,
        schedule.repeat_until,
    );
    let Some(occurrence) = rule.latest_at_or_before(now.as_millis() + lead) else {
        return Ok(false);
    };
    if now.as_millis() - (occurrence - lead) > GRACE_MILLIS {
        return Ok(false);
    }
    let ends_at = schedule
        .end_at
        .map(|end| occurrence + (end - schedule.start_at));
    if ends_at.is_some_and(|end| now.as_millis() >= end) {
        return Ok(false);
    }

    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    // 同一场已经提醒过：主键撞上，什么都不改。
    let suppressed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM planner_import_baselines WHERE account_id=? AND schedule_id=? AND skip_through_at>=?)")
        .bind(&schedule.account_id).bind(&schedule.id).bind(occurrence).fetch_one(&mut *tx).await?;
    if suppressed {
        tx.rollback().await?;
        return Ok(false);
    }
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO notifications \
         (id, account_id, kind, title, body, schedule_id, reminder_id, occurrence_at, raised_at) \
         VALUES (?, ?, 'schedule', ?, ?, ?, NULL, ?, ?)",
    )
    .bind(format!("{}-{occurrence}", schedule.id))
    .bind(&schedule.account_id)
    .bind(&schedule.title)
    .bind(schedule_body(&schedule.content()))
    .bind(&schedule.id)
    .bind(occurrence)
    .bind(now.as_millis())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(inserted > 0)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use assemble_protocol::device::NewDevice;
    use assemble_protocol::events::Event;
    use assemble_protocol::primitives::Platform;
    use assemble_protocol::schedule::{
        Planner, ReminderContent, ReminderStatus, Repeat, ScheduleContent, ScheduleStatus,
    };
    use assemble_store::secret::SecretString;
    use axum::body::{Bytes, to_bytes};
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::response::Response;
    use chrono::{FixedOffset, TimeZone};

    use super::*;
    use crate::auth::Identity;
    use crate::config::Config;
    use crate::events::Message;
    use crate::planner::{reminders, schedules};

    /// 北京时间某年某月某日某时某分的毫秒时间戳。用 chrono 独立算出，
    /// 不依赖被测代码里的日历换算。
    fn beijing(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        FixedOffset::east_opt(8 * 3600)
            .unwrap()
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
            .timestamp_millis()
    }

    /// 2026-11-25 上午九点，一个固定的"今天"。
    fn nine() -> i64 {
        beijing(2026, 11, 25, 9, 0)
    }

    /// 一天的毫秒数。
    const DAY: i64 = 24 * 60 * 60 * 1000;

    struct World {
        /// 临时目录必须活得和数据库一样久。
        _dir: tempfile::TempDir,
        state: Shared,
        identity: Identity,
    }

    /// 一个账户、一台设备，别的什么都没有。
    async fn new_world() -> World {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        let account = crate::accounts::create(
            &pool,
            "alex",
            "Alex",
            SecretString::new("a-long-enough-password".to_owned()),
        )
        .await
        .unwrap();
        let token = crate::secrets::random_token().unwrap();
        let device = crate::devices::register(
            &pool,
            &account.id,
            &NewDevice {
                name: "笔记本".into(),
                platform: Platform::Macos,
            },
            &crate::secrets::token_digest(token.expose()),
        )
        .await
        .unwrap();
        let state = Arc::new(crate::http::AppState::new(
            pool,
            Config::development(dir.path()),
        ));
        World {
            _dir: dir,
            state,
            identity: Identity {
                account_id: account.id,
                device_id: device.id,
                device_name: "笔记本".into(),
            },
        }
    }

    /// 处理器的应答里带着的那条记录。
    async fn record(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// 走真实的处理器新建一条日程，返回它的标识。
    async fn schedule(world: &World, content: ScheduleContent) -> String {
        let body = Bytes::from(serde_json::to_vec(&content).unwrap());
        let response = schedules::create(
            State(Arc::clone(&world.state)),
            world.identity.clone(),
            HeaderMap::new(),
            body,
        )
        .await
        .unwrap();
        record(response).await["id"].as_str().unwrap().to_owned()
    }

    async fn reminder(world: &World, title: &str, due: i64) -> String {
        let content = ReminderContent {
            title: title.into(),
            due_at: Timestamp::from_millis(due).unwrap(),
            notes: None,
        };
        let body = Bytes::from(serde_json::to_vec(&content).unwrap());
        let response = reminders::create(
            State(Arc::clone(&world.state)),
            world.identity.clone(),
            HeaderMap::new(),
            body,
        )
        .await
        .unwrap();
        record(response).await["id"].as_str().unwrap().to_owned()
    }

    /// 每天九点（从 `first` 那一天起）的日程。
    fn daily_at(first: i64) -> ScheduleContent {
        ScheduleContent {
            title: "周会".into(),
            location: None,
            start_at: Timestamp::from_millis(first).unwrap(),
            end_at: None,
            repeat: Repeat::Daily,
            repeat_until: None,
            remind_before: None,
            notes: None,
        }
    }

    fn once_at(start: i64) -> ScheduleContent {
        ScheduleContent {
            repeat: Repeat::None,
            ..daily_at(start)
        }
    }

    /// 名单里此刻能看到的通知编号。
    async fn notification_ids(world: &World, now: i64) -> Vec<String> {
        super::super::planner_of(
            &world.state.pool,
            &world.identity.account_id,
            Timestamp::from_millis(now).unwrap(),
        )
        .await
        .unwrap()
        .notifications
        .into_iter()
        .map(|notification| notification.id)
        .collect()
    }

    async fn tick(world: &World, millis: i64) -> Report {
        run_once(&world.state, Timestamp::from_millis(millis).unwrap())
            .await
            .unwrap()
    }

    async fn schedule_tick(world: &World, millis: i64) -> u32 {
        tick(world, millis).await.schedule_notifications
    }

    async fn reminder_tick(world: &World, millis: i64) -> u32 {
        tick(world, millis).await.reminders_fired
    }

    /// 取消一条日程：走真实的处理器。
    async fn cancel(world: &World, id: &str, version: i64) {
        let input = assemble_protocol::schedule::Versioned { version };
        let body = Bytes::from(serde_json::to_vec(&input).unwrap());
        let axum::Json(cancelled) = schedules::cancel(
            State(Arc::clone(&world.state)),
            world.identity.clone(),
            axum::extract::Path(id.to_owned()),
            body,
        )
        .await
        .unwrap();
        assert_eq!(cancelled.status, ScheduleStatus::Cancelled);
    }

    #[tokio::test]
    async fn a_reminder_fires_once_at_its_moment_and_however_late() {
        let world = new_world().await;
        let id = reminder(&world, "取快递", nine()).await;

        assert_eq!(reminder_tick(&world, nine() - 1).await, 0);
        assert_eq!(reminder_tick(&world, nine()).await, 1);
        // 再查多少遍都只有一条。
        assert_eq!(reminder_tick(&world, nine() + 1).await, 0);
        assert_eq!(reminder_tick(&world, nine() + DAY).await, 0);
        assert_eq!(
            notification_ids(&world, nine() + DAY).await,
            vec![format!("n-{id}")]
        );

        // 晚了三天才第一次检查：照样发。它是一件要做的事，晚到也比不到好。
        reminder(&world, "早就该发了", nine() - 3 * DAY).await;
        assert_eq!(reminder_tick(&world, nine() + DAY + 1).await, 1);
    }

    #[tokio::test]
    async fn a_dismissed_reminder_never_fires() {
        let world = new_world().await;
        let id = reminder(&world, "取快递", nine()).await;
        let input = assemble_protocol::schedule::Versioned { version: 1 };
        let body = Bytes::from(serde_json::to_vec(&input).unwrap());
        let axum::Json(dismissed) = reminders::dismiss(
            State(Arc::clone(&world.state)),
            world.identity.clone(),
            axum::extract::Path(id.clone()),
            body,
        )
        .await
        .unwrap();
        assert_eq!(dismissed.status, ReminderStatus::Dismissed);

        assert_eq!(reminder_tick(&world, nine()).await, 0);
        assert!(notification_ids(&world, nine()).await.is_empty());
    }

    #[tokio::test]
    async fn every_occurrence_of_a_repeating_schedule_is_announced_once() {
        let world = new_world().await;
        let id = schedule(&world, daily_at(nine())).await;

        assert_eq!(schedule_tick(&world, nine() - 1).await, 0);
        assert_eq!(schedule_tick(&world, nine()).await, 1);
        // 宽限之内再查，不重复；下午再查，这一场已经过去了。
        assert_eq!(schedule_tick(&world, nine() + 9 * 60 * 1000).await, 0);
        assert_eq!(schedule_tick(&world, nine() + 6 * 60 * 60 * 1000).await, 0);
        // 第二天同一时刻，是另一场。
        assert_eq!(schedule_tick(&world, nine() + DAY).await, 1);
        assert_eq!(
            notification_ids(&world, nine() + DAY).await,
            vec![format!("{id}-{}", nine() + DAY), format!("{id}-{}", nine()),]
        );
    }

    #[tokio::test]
    async fn monthly_occurrences_land_on_the_last_day_of_a_short_month() {
        let world = new_world().await;
        let id = schedule(
            &world,
            ScheduleContent {
                repeat: Repeat::Monthly,
                ..once_at(beijing(2026, 1, 31, 9, 0))
            },
        )
        .await;
        let feb = beijing(2026, 2, 28, 9, 0);
        let march = beijing(2026, 3, 31, 9, 0);
        let april = beijing(2026, 4, 30, 9, 0);

        // 月末那一场连着查两遍：只通知一次。
        assert_eq!(schedule_tick(&world, feb).await, 1);
        assert_eq!(schedule_tick(&world, feb + 1).await, 0);
        assert_eq!(schedule_tick(&world, march).await, 1);
        assert_eq!(schedule_tick(&world, april).await, 1);
        // 三场各一条，编号都按各自的场次时刻算。
        let ids: HashSet<String> = sqlx::query_scalar::<_, String>("SELECT id FROM notifications")
            .fetch_all(&world.state.pool)
            .await
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(
            ids,
            HashSet::from_iter([
                format!("{id}-{feb}"),
                format!("{id}-{march}"),
                format!("{id}-{april}"),
            ])
        );
    }

    #[tokio::test]
    async fn a_moment_that_passed_too_long_ago_is_not_announced() {
        let world = new_world().await;
        schedule(&world, daily_at(nine())).await;
        // 九点过十分零一毫秒才第一次检查：这一场不再提醒。
        assert_eq!(schedule_tick(&world, nine() + GRACE_MILLIS + 1).await, 0);

        // 宽限的最后一毫秒还算。
        let world = new_world().await;
        schedule(&world, daily_at(nine())).await;
        assert_eq!(schedule_tick(&world, nine() + GRACE_MILLIS).await, 1);
    }

    #[tokio::test]
    async fn the_end_of_an_occurrence_closes_the_window() {
        let world = new_world().await;
        schedule(
            &world,
            ScheduleContent {
                end_at: Some(Timestamp::from_millis(nine() + 5 * 60 * 1000).unwrap()),
                ..once_at(nine())
            },
        )
        .await;
        // 这一场开了四分钟：还在宽限里，也没结束，提醒。
        assert_eq!(schedule_tick(&world, nine() + 4 * 60 * 1000).await, 1);

        // 另一条五分钟就结束的：第六分钟才看到它，不提醒。
        let world = new_world().await;
        schedule(
            &world,
            ScheduleContent {
                end_at: Some(Timestamp::from_millis(nine() + 5 * 60 * 1000).unwrap()),
                ..once_at(nine())
            },
        )
        .await;
        assert_eq!(schedule_tick(&world, nine() + 6 * 60 * 1000).await, 0);
    }

    #[tokio::test]
    async fn a_lead_time_moves_the_moment_earlier() {
        let world = new_world().await;
        let id = schedule(
            &world,
            ScheduleContent {
                remind_before: Some(30),
                ..once_at(nine())
            },
        )
        .await;
        // 八点半该提醒，八点二十九还不该。
        assert_eq!(schedule_tick(&world, nine() - 30 * 60 * 1000 - 1).await, 0);
        assert_eq!(schedule_tick(&world, nine() - 30 * 60 * 1000).await, 1);
        // 到了九点也不再来一遍：通知编号是按场次算的。
        assert_eq!(schedule_tick(&world, nine()).await, 0);
        assert_eq!(
            notification_ids(&world, nine()).await,
            vec![format!("{id}-{}", nine())]
        );
    }

    #[tokio::test]
    async fn a_finished_or_cancelled_schedule_stays_quiet() {
        let world = new_world().await;
        let id = schedule(&world, daily_at(nine())).await;
        cancel(&world, &id, 1).await;
        assert_eq!(schedule_tick(&world, nine()).await, 0);

        // 重复到昨天为止的日程：今天没有场次。
        let world = new_world().await;
        schedule(
            &world,
            ScheduleContent {
                repeat_until: Some(Timestamp::from_millis(nine() - DAY).unwrap()),
                ..daily_at(nine() - 3 * DAY)
            },
        )
        .await;
        assert_eq!(schedule_tick(&world, nine()).await, 0);
    }

    #[tokio::test]
    async fn checking_the_same_moment_twice_produces_one_notification() {
        let world = new_world().await;
        let id = schedule(&world, daily_at(nine())).await;
        assert_eq!(schedule_tick(&world, nine()).await, 1);
        assert_eq!(schedule_tick(&world, nine()).await, 0);
        assert_eq!(
            notification_ids(&world, nine()).await,
            vec![format!("{id}-{}", nine())]
        );
    }

    #[tokio::test]
    async fn a_new_repeating_schedule_does_not_dig_up_its_past() {
        let world = new_world().await;
        // 昨天、前天就开始了的日程，今天上午九点零五才建进来：
        // 最近的一场是今早九点，提醒时刻五分钟前，还在宽限里，提醒一次。
        schedule(&world, daily_at(nine() - 2 * DAY)).await;
        assert_eq!(schedule_tick(&world, nine() + 5 * 60 * 1000).await, 1);
        // 要是下午才建进来，今早那一场早就过了，不翻旧账；明天的照常。
        let world = new_world().await;
        schedule(&world, daily_at(nine() - 2 * DAY)).await;
        assert_eq!(schedule_tick(&world, nine() + 6 * 60 * 60 * 1000).await, 0);
        assert_eq!(schedule_tick(&world, nine() + DAY).await, 1);
    }

    #[tokio::test]
    async fn each_round_pushes_the_planner_to_the_devices_of_the_account() {
        let world = new_world().await;
        let id = reminder(&world, "取快递", nine()).await;
        // 新建本身就推过一次；从现在开始听。
        let mut receiver = world.state.hub.subscribe(&world.identity.account_id);

        // 不到点的一轮什么都不推。
        assert_eq!(reminder_tick(&world, nine() - 1).await, 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), receiver.recv())
                .await
                .is_err()
        );

        assert_eq!(reminder_tick(&world, nine()).await, 1);
        let message = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        let Message::Event {
            audience: None,
            event,
        } = message.as_ref()
        else {
            panic!("a planner event should be broadcast to the whole account");
        };
        let Event::PlannerChanged { planner } = event.as_ref() else {
            panic!("the event should carry the planner");
        };
        assert_eq!(planner.notifications.len(), 1);
        assert_eq!(planner.notifications[0].id, format!("n-{id}"));
        assert_eq!(planner.reminders[0].status, ReminderStatus::Fired);
    }

    #[tokio::test]
    async fn a_fired_reminder_is_listed_with_its_notification() {
        let world = new_world().await;
        let id = reminder(&world, "取快递", nine()).await;
        assert_eq!(reminder_tick(&world, nine()).await, 1);
        let planner: Planner = super::super::planner_of(
            &world.state.pool,
            &world.identity.account_id,
            Timestamp::from_millis(nine()).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(planner.reminders.len(), 1);
        assert_eq!(planner.reminders[0].id, id);
        assert_eq!(planner.reminders[0].status, ReminderStatus::Fired);
        assert_eq!(planner.reminders[0].version, 2);
        assert_eq!(planner.notifications[0].title, "取快递");
        assert_eq!(
            planner.notifications[0].reminder_id.as_deref(),
            Some(id.as_str())
        );
        assert_eq!(planner.notifications[0].occurrence_at, None);
    }
}
