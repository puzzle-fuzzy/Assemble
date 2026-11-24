//! 日程在桌面端的落点。
//!
//! - **名单整份来、整份换。** 服务端任何一处改动都推一份整名单过来；连上、以及定期对账时
//!   再拉一次。这里不做增量合并。
//! - **到点的通知弹系统通知，一条只弹一次。** 名单里带着全部还没读的通知；哪几条已经弹过，
//!   记在本机设置的有界账本里。断线期间到的、服务端停机时错过的，重新上线拿到名单就补弹：
//!   只补最近二十四小时之内生成的，再早的只留在列表里。
//! - **点开通知，主窗口打开并定位到那一条。** 通知里只有标题与几句话；它是哪一条日程或提醒，
//!   由 Rust 按通知的标识找回。
//! - **重复的场次在这里展开。** 界面要显示"接下来几场"，用的是和服务端同一个规则库
//!   （`assemble-schedule`），所以界面上的时间和实际提醒的时间对得上。

use std::time::Duration;

use assemble_client::{ClientError, RequestOptions};
use assemble_protocol::primitives::Timestamp;
use assemble_protocol::schedule::{
    Location, Notification, Planner, ReminderStatus, Repeat, Schedule, ScheduleStatus,
};
use assemble_schedule::occurrences::Recurrence;
use chrono::{Datelike, TimeZone};
use serde::{Deserialize, Serialize};
use tauri::Manager;
use tokio_util::sync::CancellationToken;

use crate::state::AppState;

/// 回头多久之内生成的通知要补弹：二十四小时。再早的只留在列表里。
pub const CATCH_UP_WINDOW: Duration = Duration::from_hours(24);

/// 名单往前看多少天。
pub const AGENDA_DAYS: i64 = 7;

/// 每条日程最多展开多少场：天天有的日程，七天也就七场。
const OCCASIONS: usize = 16;

/// 拉一次名单最多等多久。
const SHORT_TIMEOUT: Duration = Duration::from_secs(6);

const MINUTE: i64 = 60 * 1000;

/// 用户现在要写的东西：从托盘的"新建提醒…"进来时，主窗口直接摆出那张表单。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Compose {
    Reminder,
    Schedule,
}

/// 日程面板在界面上的样子。可选的字段一律写出来：没有就是 `null`，不省略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerView {
    /// 还没读的通知，新的在前。
    pub notifications: Vec<NotificationView>,
    /// 还在进行中的日程，按最近的一场排在前面。
    pub schedules: Vec<ScheduleView>,
    /// 还没办完的提醒，按到点时间排。
    pub reminders: Vec<ReminderView>,
    /// 点开某条通知之后要定位到那一条；没有要定位的时为 `None`。
    pub focus: Option<String>,
    /// 现在要摆出哪张新建表单；没有时为 `None`。
    pub compose: Option<Compose>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationView {
    pub id: String,
    pub title: String,
    pub body: Option<String>,
    /// 它是哪一条日程或提醒；界面上据此把通知与列表里的那一项对上。
    pub schedule_id: Option<String>,
    pub reminder_id: Option<String>,
    pub raised_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleView {
    pub id: String,
    pub version: i64,
    /// 建立它的那台设备当时叫什么：助手写的和手建的，在界面上分得开。
    pub created_by: String,
    pub title: String,
    pub location: Option<Location>,
    pub start_at: Timestamp,
    pub end_at: Option<Timestamp>,
    pub repeat: Repeat,
    pub repeat_until: Option<Timestamp>,
    /// 提前多少分钟提醒；`None` 就是到点才提醒。
    pub remind_before: Option<u32>,
    pub notes: Option<String>,
    /// 从今天起、七天之内会出现的几场，含今天已经过去的那些。
    pub occurrences: Vec<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderView {
    pub id: String,
    pub version: i64,
    pub status: ReminderStatus,
    pub created_by: String,
    pub title: String,
    pub due_at: Timestamp,
    pub notes: Option<String>,
}

/// 把服务端的名单变成界面上的样子。只留还在进行中的：完成、取消的记录留在服务端，
/// 但不必再摆给人看。
pub fn view(planner: &Planner, focus: Option<&str>, compose: Option<Compose>) -> PlannerView {
    let since = local_day_start();
    let mut schedules: Vec<ScheduleView> = planner
        .schedules
        .iter()
        .filter(|schedule| schedule.status == ScheduleStatus::Upcoming)
        .filter_map(|schedule| {
            let occurrences = occurrences_of(schedule, since);
            // 七天里一场都没有的（比如每周一次、但已经过了这个星期的那一天）：不占地方。
            (!occurrences.is_empty()).then(|| ScheduleView {
                id: schedule.id.clone(),
                version: schedule.version,
                created_by: schedule.created_by.clone(),
                title: schedule.content.title.clone(),
                location: schedule.content.location.clone(),
                start_at: schedule.content.start_at,
                end_at: schedule.content.end_at,
                repeat: schedule.content.repeat,
                repeat_until: schedule.content.repeat_until,
                remind_before: schedule.content.remind_before,
                notes: schedule.content.notes.clone(),
                occurrences,
            })
        })
        .collect();
    // 最近的一场在前的排前面；一样早的按标题，保证每次的次序一样。
    schedules.sort_by(|a, b| {
        a.occurrences
            .first()
            .cmp(&b.occurrences.first())
            .then_with(|| a.title.cmp(&b.title))
    });

    let mut reminders: Vec<ReminderView> = planner
        .reminders
        .iter()
        .filter(|reminder| {
            matches!(
                reminder.status,
                ReminderStatus::Scheduled | ReminderStatus::Fired
            )
        })
        .map(|reminder| ReminderView {
            id: reminder.id.clone(),
            version: reminder.version,
            status: reminder.status,
            created_by: reminder.created_by.clone(),
            title: reminder.content.title.clone(),
            due_at: reminder.content.due_at,
            notes: reminder.content.notes.clone(),
        })
        .collect();
    reminders.sort_by(|a, b| a.due_at.cmp(&b.due_at).then_with(|| a.title.cmp(&b.title)));

    PlannerView {
        notifications: planner
            .notifications
            .iter()
            .map(|notification| NotificationView {
                id: notification.id.clone(),
                title: notification.title.clone(),
                body: notification.body.clone(),
                schedule_id: notification.schedule_id.clone(),
                reminder_id: notification.reminder_id.clone(),
                raised_at: notification.raised_at,
            })
            .collect(),
        schedules,
        reminders,
        focus: focus.map(str::to_owned),
        compose,
    }
}

/// 一条日程从今天起七天之内会出现的几场。
fn occurrences_of(schedule: &Schedule, since: i64) -> Vec<Timestamp> {
    let rule = Recurrence::new(
        schedule.content.start_at.as_millis(),
        schedule.content.repeat,
        schedule.content.repeat_until.map(Timestamp::as_millis),
    );
    let until = since + AGENDA_DAYS * 24 * 60 * MINUTE;
    rule.upcoming(since, OCCASIONS)
        .into_iter()
        .filter(|at| *at <= until)
        .filter_map(Timestamp::from_millis)
        .collect()
}

/// 本机时区下"今天"从哪一刻开始。
fn local_day_start() -> i64 {
    let today = chrono::Local::now().date_naive();
    let midnight = today.and_hms_opt(0, 0, 0).unwrap_or_default();
    match chrono::Local.from_local_datetime(&midnight).single() {
        Some(start) => start.timestamp_millis(),
        // 夏令时切换的那一小时里"零点"可能不存在：退回 UTC 的零点，只影响边界上的一场。
        None => midnight.and_utc().timestamp_millis(),
    }
}

/// 本机时区下今天是第几天（从公元元年数起）。过了零点它会变。
pub fn local_day() -> i32 {
    chrono::Local::now().date_naive().num_days_from_ce()
}

/// 接到一份名单：换上它，再把该弹的弹出去。
pub fn arrived<R: tauri::Runtime>(state: &AppState<R>, planner: Planner) {
    announce(state, &planner);
    state.set_planner(Some(planner));
}

/// 拉一次名单。
pub async fn refresh<R: tauri::Runtime>(
    state: &AppState<R>,
    cancel: &CancellationToken,
) -> Result<(), ClientError> {
    let planner = state
        .client
        .planner(&RequestOptions {
            cancel,
            timeout: Some(SHORT_TIMEOUT),
        })
        .await?;
    arrived(state, planner);
    Ok(())
}

/// 名单里还没读的通知，哪些是这台电脑没弹过的：弹出去，并记下来。
///
/// 只补最近二十四小时之内生成的。设了"不弹提醒"的电脑也照样记：以后打开提醒时，
/// 不该把积压的全弹一遍。
fn announce<R: tauri::Runtime>(state: &AppState<R>, planner: &Planner) {
    let now = Timestamp::now();
    let fresh: Vec<Notification> = planner
        .notifications
        .iter()
        .filter(|notification| now.elapsed_since(notification.raised_at) <= CATCH_UP_WINDOW)
        .cloned()
        .collect();
    if fresh.is_empty() {
        return;
    }
    let (unannounced, muted) = state.edit_settings(|settings| {
        let unannounced: Vec<Notification> = fresh
            .into_iter()
            .filter(|notification| settings.delivered.record(&notification.id))
            .collect();
        (unannounced, settings.planner_muted)
    });
    if unannounced.is_empty() {
        return;
    }
    if let Err(error) = crate::transfers::save_settings(state.app(), &state.settings()) {
        // 弹是照弹的，只是没记下来：重启之后可能再弹一次。不至于为它停下。
        tracing::warn!(%error, "the delivered reminders could not be saved");
    }
    if muted {
        return;
    }
    for notification in unannounced {
        let handle = state.fresh_handle();
        state.track_notification(handle, &notification.id);
        state.notify_reminder(handle, &notification.title, &words(&notification));
    }
}

/// 通知底下的几句话：日程先说几点开始，再是备注与地点。
fn words(notification: &Notification) -> String {
    let mut lines = Vec::new();
    if let Some(at) = notification.occurrence_at {
        lines.push(format!("{} 开始", local_time(at)));
    }
    if let Some(body) = &notification.body {
        lines.push(body.clone());
    }
    lines.join("\n")
}

/// 本机时区下的"月日 时:分"。
fn local_time(at: Timestamp) -> String {
    match chrono::Local.timestamp_millis_opt(at.as_millis()).single() {
        Some(at) => at.format("%m月%d日 %H:%M").to_string(),
        None => String::new(),
    }
}

/// 用户要从托盘里新建一条提醒或日程：主窗口打开，直接摆出那张表单。
pub fn compose<R: tauri::Runtime>(app: &tauri::AppHandle<R>, kind: Compose) {
    let state = app.state::<AppState<R>>();
    state.set_planner_compose(Some(kind));
    crate::state::show_main(app);
}

/// 用户点开了一条系统通知：主窗口打开，定位到它。
pub fn focus<R: tauri::Runtime>(app: &tauri::AppHandle<R>, notification_id: &str) {
    let state = app.state::<AppState<R>>();
    state.set_planner_focus(Some(notification_id.to_owned()));
    crate::state::show_main(app);
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::state::{Notifier, Settings};
    use assemble_protocol::schedule::Reminder;

    /// 把弹出来的通知记下来，不真的弹：跑测试不该在屏幕上冒通知。
    #[derive(Debug, Default)]
    struct RecordingNotifier(Mutex<Vec<(i32, String, String)>>);

    impl Notifier for RecordingNotifier {
        fn received(&self, _handle: i32, _peer: &str, _file_name: &str, _fell_back: bool) {}

        fn reminder(&self, handle: i32, title: &str, body: &str) {
            self.0
                .lock()
                .unwrap()
                .push((handle, title.to_owned(), body.to_owned()));
        }
    }

    /// 一个固定的时刻：给的是带偏移的时间，和机器在哪个时区无关。
    fn local(text: &str) -> Timestamp {
        let parsed = chrono::DateTime::parse_from_rfc3339(text).unwrap();
        Timestamp::from_millis(parsed.timestamp_millis()).unwrap()
    }

    /// 本机时区下的"年月日 时:分"换成的时刻。`local_time` 也是按本机时区写的，
    /// 两个凑在一起，测试在哪个时区的机器上都一样。
    fn local_clock(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> Timestamp {
        let at = chrono::Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .unwrap();
        Timestamp::from_millis(at.timestamp_millis()).unwrap()
    }

    fn planner() -> Planner {
        Planner {
            schedules: Vec::new(),
            reminders: Vec::new(),
            notifications: vec![Notification {
                id: "note-1".into(),
                kind: assemble_protocol::schedule::NotificationKind::Reminder,
                title: "取快递".into(),
                body: Some("驿站楼下".into()),
                schedule_id: None,
                reminder_id: Some("r1".into()),
                occurrence_at: None,
                raised_at: Timestamp::now(),
            }],
        }
    }

    fn state_with(
        settings: Settings,
        notifier: Arc<RecordingNotifier>,
    ) -> tauri::State<'static, AppState<tauri::test::MockRuntime>> {
        let app = tauri::test::mock_app();
        let client =
            assemble_client::Client::new(assemble_protocol::origin::ServerOrigin::development())
                .unwrap();
        app.manage(AppState::new(
            app.handle().clone(),
            client,
            None,
            settings,
            std::path::PathBuf::from("/tmp/assemble-planner-tests"),
            notifier,
        ));
        // 把应用装进一个活得和测试一样久的地方。
        Box::leak(Box::new(app)).state()
    }

    fn shown(notifier: &RecordingNotifier) -> Vec<String> {
        notifier
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|(_, title, _)| title.clone())
            .collect()
    }

    #[test]
    fn each_notification_is_shown_once_and_old_ones_are_left_in_the_list() {
        let notifier = Arc::new(RecordingNotifier::default());
        let state = state_with(Settings::default(), notifier.clone());
        let mut first = planner();

        arrived(&state, first.clone());
        assert_eq!(shown(&notifier), vec!["取快递"]);

        // 同一份名单再来一遍（比如别的设备改了一条日程）：不重复弹。
        arrived(&state, first.clone());
        assert_eq!(shown(&notifier).len(), 1);

        // 两天前生成的通知：只留在列表里，不补弹。
        first.notifications[0].raised_at = Timestamp::now() - Duration::from_hours(48);
        let mut overdue = first.clone();
        overdue.notifications[0].id = "note-old".into();
        arrived(&state, overdue);
        assert_eq!(shown(&notifier).len(), 1);

        // 弹过哪几条记在本机设置里：重启之后也不会把它们再弹一遍。
        let saved = state.settings().delivered;
        assert!(saved.contains("note-1"));
        assert!(!saved.contains("note-old"));
    }

    #[test]
    fn a_muted_computer_stays_quiet_but_still_remembers() {
        let notifier = Arc::new(RecordingNotifier::default());
        let settings = Settings {
            planner_muted: true,
            ..Settings::default()
        };
        let state = state_with(settings, notifier.clone());

        arrived(&state, planner());
        assert_eq!(shown(&notifier), Vec::<String>::new());
        // 以后打开提醒时，不该把这条积压的再弹一遍。
        assert!(state.settings().delivered.contains("note-1"));
    }

    #[test]
    fn occurrences_are_expanded_for_the_next_seven_days_only() {
        let nine = local("2026-11-25T09:00:00+08:00");
        let daily = assemble_protocol::schedule::Schedule {
            id: "s1".into(),
            version: 1,
            status: ScheduleStatus::Upcoming,
            created_by: "本机".into(),
            created_at: nine,
            updated_at: nine,
            content: assemble_protocol::schedule::ScheduleContent {
                title: "周会".into(),
                location: None,
                start_at: nine,
                end_at: None,
                repeat: Repeat::Daily,
                repeat_until: None,
                remind_before: None,
                notes: None,
            },
        };
        let expanded = occurrences_of(&daily, nine.as_millis() - 9 * 60 * MINUTE);
        // 从那天零点起七天：第一天到第七天各一场，第八天不算。
        assert_eq!(expanded.len(), 7);
        assert_eq!(expanded[0], nine);
        assert_eq!(
            expanded[6],
            Timestamp::from_millis(nine.as_millis() + 6 * 24 * 60 * MINUTE).unwrap()
        );

        // 不重复的日程只有一场；早就过去的也不出现在名单里。
        let once = assemble_protocol::schedule::Schedule {
            content: assemble_protocol::schedule::ScheduleContent {
                repeat: Repeat::None,
                ..daily.content.clone()
            },
            ..daily.clone()
        };
        assert_eq!(occurrences_of(&once, nine.as_millis() - MINUTE), vec![nine]);
        assert_eq!(occurrences_of(&once, nine.as_millis() + MINUTE), Vec::new());

        // 完成的日程不进名单。
        let done = assemble_protocol::schedule::Schedule {
            status: ScheduleStatus::Done,
            ..once
        };
        let listing = Planner {
            schedules: vec![done],
            reminders: Vec::new(),
            notifications: Vec::new(),
        };
        assert_eq!(view(&listing, None, None).schedules, Vec::new());
    }

    #[test]
    fn reminders_on_the_list_are_the_unfinished_ones_in_order() {
        let at = local("2026-11-25T01:00:00+08:00");
        let reminder = |id: &str, status: ReminderStatus, hour: u32| {
            let due =
                Timestamp::from_millis(at.as_millis() + i64::from(hour) * 60 * MINUTE).unwrap();
            Reminder {
                id: id.into(),
                version: 1,
                status,
                created_by: "ZCode 的助手".into(),
                created_at: due,
                updated_at: due,
                content: assemble_protocol::schedule::ReminderContent {
                    title: format!("提醒 {id}"),
                    due_at: due,
                    notes: None,
                },
            }
        };
        let listing = Planner {
            schedules: Vec::new(),
            reminders: vec![
                reminder("late", ReminderStatus::Scheduled, 5),
                reminder("gone", ReminderStatus::Dismissed, 1),
                reminder("soon", ReminderStatus::Fired, 2),
            ],
            notifications: Vec::new(),
        };
        let shown = view(&listing, Some("note-9"), None);
        assert_eq!(
            shown
                .reminders
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            vec!["soon", "late"]
        );
        assert_eq!(shown.reminders[1].status, ReminderStatus::Scheduled);
        assert_eq!(shown.focus.as_deref(), Some("note-9"));
    }

    #[test]
    fn a_click_on_a_notification_maps_back_to_it() {
        let notifier = Arc::new(RecordingNotifier::default());
        let state = state_with(Settings::default(), notifier.clone());
        arrived(&state, planner());
        let shown = notifier.0.lock().unwrap().clone();
        assert_eq!(shown.len(), 1);
        let handle = shown[0].0;
        assert_eq!(
            state.notification_for_handle(handle).as_deref(),
            Some("note-1")
        );
        // 不是日程的通知（比如"收到文件"）不在这里：找得到文件，找不到日程。
        assert_eq!(state.notification_for_handle(handle + 100), None);

        // 定位到它；它已读之后定位自动清掉。
        state.set_planner_focus(Some("note-1".into()));
        assert_eq!(
            state.snapshot().planner.unwrap().focus.as_deref(),
            Some("note-1")
        );
        let read = Planner {
            notifications: Vec::new(),
            ..planner()
        };
        state.set_planner(Some(read));
        assert_eq!(state.snapshot().planner.unwrap().focus, None);
    }

    #[test]
    fn signing_out_clears_the_list() {
        let notifier = Arc::new(RecordingNotifier::default());
        let state = state_with(Settings::default(), notifier.clone());
        arrived(&state, planner());
        assert!(state.snapshot().planner.is_some());
        // 托盘里点过"新建提醒…"，表单还没关就退出了。
        state.set_planner_compose(Some(Compose::Reminder));
        state.end_session();
        assert!(state.snapshot().planner.is_none());
        // 下一个登录的人拿到名单时，不该一进来就看到那张表单。
        arrived(&state, planner());
        assert_eq!(state.snapshot().planner.unwrap().compose, None);
    }

    #[test]
    fn the_words_under_a_notification_name_when_it_starts() {
        // 按本机时区写下"九点"：`words` 也是按本机时区写的，机器在哪个时区都该是九点。
        let at = local_clock(2026, 11, 25, 9, 0);
        let mut notification = Notification {
            id: "s1-1".into(),
            kind: assemble_protocol::schedule::NotificationKind::Schedule,
            title: "火车".into(),
            body: Some("带身份证".into()),
            schedule_id: Some("s1".into()),
            reminder_id: None,
            occurrence_at: Some(at),
            raised_at: at,
        };
        let text = words(&notification);
        assert!(text.starts_with("11月25日 09:00 开始"), "{text}");
        assert!(text.ends_with("带身份证"));
        notification.occurrence_at = None;
        assert_eq!(words(&notification), "带身份证");
    }
}
