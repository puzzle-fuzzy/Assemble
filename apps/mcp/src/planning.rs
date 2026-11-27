//! 把名单变成给助手看的文字，以及建日程之前要算的几件事。
//!
//! 这里都是纯函数，不联网、不读钥匙串。"现在"由调用方传进来，所以测试可以拨着表跑。
//!
//! 文字是写给模型的：时间一律写明白的日期和星期，不用"明天""下周"这种要看上下文的说法；
//! 每一条都带着标识和版本号，助手下一步要改、要取消时直接用。

use assemble_protocol::account::Account;
use assemble_protocol::device::Device;
use assemble_protocol::primitives::Timestamp;
use assemble_protocol::schedule::{
    Location, Notification, Planner, Reminder, ReminderStatus, Repeat, Schedule, ScheduleContent,
};
use chrono::{DateTime, Datelike, Local, NaiveDateTime, TimeZone, Timelike, Weekday};

const MINUTE: i64 = 60 * 1000;
const DAY: i64 = 24 * 60 * MINUTE;

/// 一条重复的日程最多点名几场，再多的只报个数。
const SHOWN_OCCURRENCES: usize = 3;

/// 撞车最多点名几处，再多的只报个数。
const SHOWN_CONFLICTS: usize = 5;

/// 工具收的时间不对。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeProblem {
    /// 写法不是 ISO 8601 的日期时间。
    NotADateTime,
    /// 日期和时刻都对，只是没带时区。"明天 9:00"在不同时区是不同的时刻，不能猜。
    NoTimeZone,
    /// 写法对，但超出了可以表示的范围，多半是把年份填错了。
    OutOfRange,
}

impl TimeProblem {
    /// 给助手看的一句话。`what` 是"开始时间"这样的名字。
    pub fn message(self, what: &str) -> String {
        match self {
            Self::NotADateTime => format!(
                "{what}的写法不对：要像 2026-11-26T09:00:00+08:00 这样，带日期、时刻和时区。"
            ),
            Self::NoTimeZone => {
                format!("{what}没有带时区：要像 2026-11-26T09:00:00+08:00 这样写上时区，不能猜。")
            }
            Self::OutOfRange => {
                format!("{what}超出了可以表示的范围，年份是不是填错了？")
            }
        }
    }
}

/// 解析一个带时区的 ISO 8601 时间，换成服务端用的 UTC 时刻。
pub fn parse_time(text: &str) -> Result<Timestamp, TimeProblem> {
    let text = text.trim();
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(text) {
        return Timestamp::from_millis(parsed.timestamp_millis()).ok_or(TimeProblem::OutOfRange);
    }
    // 日期和时刻都对、只是没带时区：单独说清楚，别让它混在"写法不对"里。
    let naive = ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"]
        .iter()
        .find_map(|pattern| NaiveDateTime::parse_from_str(text, pattern).ok());
    match naive {
        Some(_) => Err(TimeProblem::NoTimeZone),
        None => Err(TimeProblem::NotADateTime),
    }
}

/// 看名单的那个时刻。时间都按这台电脑的时区来说。
#[derive(Debug, Clone, Copy)]
pub struct Now {
    local: DateTime<Local>,
    now_ms: i64,
}

impl Now {
    pub fn at(local: DateTime<Local>) -> Self {
        Self {
            now_ms: local.timestamp_millis(),
            local,
        }
    }

    pub fn now(&self) -> i64 {
        self.now_ms
    }

    /// 本机时区下"今天"从哪一刻开始。
    pub fn today_start(&self) -> i64 {
        let midnight = self
            .local
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap_or_default();
        match Local.from_local_datetime(&midnight).single() {
            Some(start) => start.timestamp_millis(),
            // 夏令时切换的那一小时里"零点"可能不存在：退回 UTC 的零点，只影响边界上的一场。
            None => midnight.and_utc().timestamp_millis(),
        }
    }

    /// 从今天起 `days` 天之后的那一刻：名单看到的最后一天刚刚结束的时候。
    pub fn days_ahead(&self, days: u32) -> i64 {
        self.today_start() + i64::from(days) * DAY
    }

    /// 给人看的时区，比如 `+08:00`。
    fn offset(&self) -> String {
        let seconds = self.local.offset().local_minus_utc();
        let (sign, rest) = if seconds < 0 {
            ('-', -seconds)
        } else {
            ('+', seconds)
        };
        format!("{sign}{:02}:{:02}", rest / 3600, rest % 3600 / 60)
    }
}

fn weekday(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "周一",
        Weekday::Tue => "周二",
        Weekday::Wed => "周三",
        Weekday::Thu => "周四",
        Weekday::Fri => "周五",
        Weekday::Sat => "周六",
        Weekday::Sun => "周日",
    }
}

/// 一个毫秒时刻在本机时区下的样子。超出 `Local` 能表示的范围时是它的起点，不会更糟。
fn local_time(at_ms: i64) -> DateTime<Local> {
    Local
        .timestamp_millis_opt(at_ms)
        .single()
        .unwrap_or_default()
}

/// `2026-11-25 周三 09:00`。
fn stamp(at_ms: i64) -> String {
    let at = local_time(at_ms);
    format!("{} {:02}:{:02}", date_part(at_ms), at.hour(), at.minute())
}

/// `2026-11-25 周三`。
fn date_part(at_ms: i64) -> String {
    let at = local_time(at_ms);
    format!(
        "{:04}-{:02}-{:02} {}",
        at.year(),
        at.month(),
        at.day(),
        weekday(at.weekday())
    )
}

fn rule_of(content: &ScheduleContent) -> assemble_schedule::occurrences::Recurrence {
    assemble_schedule::occurrences::Recurrence::new(
        content.start_at.as_millis(),
        content.repeat,
        content.repeat_until.map(Timestamp::as_millis),
    )
}

/// 一条日程在一段时间里会出现的几场，按时间排，含已经过去的。
pub fn occurrences_of(schedule: &Schedule, since_ms: i64, until_ms: i64) -> Vec<i64> {
    rule_of(&schedule.content)
        .upcoming(since_ms, 64)
        .into_iter()
        .take_while(|at| *at <= until_ms)
        .collect()
}

/// 一场持续多久。没写结束时间的，就当它只占开始的那一瞬。
fn length_of(content: &ScheduleContent) -> i64 {
    content
        .end_at
        .map_or(0, |end| end.as_millis() - content.start_at.as_millis())
}

/// 两段时间有没有交叠。首尾相接的两场不算。
fn overlaps(a_start: i64, a_length: i64, b_start: i64, b_length: i64) -> bool {
    // 只占一瞬的那一场多算一毫秒，好让同一时刻的两个点、落在别人时段里的点都算撞上。
    let (a_end, b_end) = (a_start + a_length.max(1), b_start + b_length.max(1));
    a_start < b_end && b_start < a_end
}

/// 和别的安排撞上的一场。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub title: String,
    /// 撞上的那一场的开始时刻。
    pub at_ms: i64,
}

/// 一份日程内容会不会和名单里别的日程撞车。
///
/// 从它第一场（或者昨天，取晚的那个）往后看七天：它的每一场和别的日程的每一场比时间。
/// `skip` 是正在修改的那一条的标识：别让它和自己比。
pub fn conflicts(
    content: &ScheduleContent,
    others: &[Schedule],
    now: Now,
    skip: Option<&str>,
) -> Vec<Conflict> {
    let from = content.start_at.as_millis().max(now.now() - DAY);
    let until = from + 7 * DAY;
    let mine = rule_of(content)
        .upcoming(from, 32)
        .into_iter()
        .take_while(|at| *at <= until)
        .collect::<Vec<_>>();
    let length = length_of(content);
    let mut found = Vec::new();
    for other in others {
        if skip == Some(other.id.as_str()) {
            continue;
        }
        // 别的那一场可能已经开始、还没结束：往前多看它自己的时长。
        let theirs_from = from - length_of(&other.content);
        for at in occurrences_of(other, theirs_from, until) {
            let hit = mine
                .iter()
                .any(|mine_at| overlaps(*mine_at, length, at, length_of(&other.content)));
            if hit {
                found.push(Conflict {
                    title: other.content.title.clone(),
                    at_ms: at,
                });
            }
        }
    }
    found.sort_by_key(|conflict| conflict.at_ms);
    found.dedup();
    found
}

/// 撞车的提示。没有撞车时是空串。
pub fn conflict_text(conflicts: &[Conflict]) -> String {
    match conflicts.len() {
        0 => String::new(),
        1 => format!(
            "提示：它和「{}」（{}）撞车。",
            conflicts[0].title,
            stamp(conflicts[0].at_ms)
        ),
        count if count <= SHOWN_CONFLICTS => {
            let listed: Vec<String> = conflicts
                .iter()
                .map(|conflict| format!("「{}」（{}）", conflict.title, stamp(conflict.at_ms)))
                .collect();
            format!("提示：它和 {} 撞车。", listed.join("、"))
        }
        count => {
            let listed: Vec<String> = conflicts[..SHOWN_CONFLICTS]
                .iter()
                .map(|conflict| format!("「{}」", conflict.title))
                .collect();
            format!("提示：它和{} 等 {count} 场安排撞车。", listed.join("、"))
        }
    }
}

/// 地点、提前量和备注：一条日程后面跟着的几句话。
fn extras(content: &ScheduleContent) -> String {
    let mut parts = Vec::new();
    if let Some(Location { label, .. }) = &content.location {
        parts.push(format!("地点：{label}"));
    }
    let minutes = content.lead_minutes();
    if minutes > 0 {
        parts.push(format!("提前 {minutes} 分钟提醒"));
    }
    if let Some(notes) = &content.notes {
        parts.push(format!("备注：{notes}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" · "))
    }
}

/// 一场的时间：几点到几点。同一天的只写一次日期；已经过去的标出来。
fn span_text(now: Now, at_ms: i64, length_ms: i64) -> String {
    let mut text = if length_ms > 0 {
        let end = local_time(at_ms + length_ms);
        let same_day = end.date_naive() == local_time(at_ms).date_naive();
        if same_day {
            format!("{} 到 {:02}:{:02}", stamp(at_ms), end.hour(), end.minute())
        } else {
            format!("{} 到 {}", stamp(at_ms), stamp(at_ms + length_ms))
        }
    } else {
        stamp(at_ms)
    };
    if at_ms + length_ms.max(1) <= now.now() {
        text.push_str("（已过）");
    }
    text
}

/// 一条日程的样子，带标识和版本号：助手下一步要改、要取消时用。
///
/// `until_ms` 是名单看到哪儿为止（不含）。重复的日程只列这之前的场次，从今天零点起算——
/// 和整份名单看的是同一个窗口。
pub fn schedule_line(now: Now, schedule: &Schedule, until_ms: i64) -> String {
    let content = &schedule.content;
    let until = content.repeat_until.map_or_else(String::new, |until| {
        format!("，到 {}", date_part(until.as_millis()))
    });
    let times = if content.repeat == Repeat::None {
        span_text(now, content.start_at.as_millis(), length_of(content))
    } else {
        let label = match content.repeat {
            Repeat::None => "",
            Repeat::Daily => "每天",
            Repeat::Weekly => "每周",
            Repeat::Monthly => "每月",
        };
        let shown: Vec<String> = occurrences_of(schedule, now.today_start(), until_ms)
            .iter()
            .map(|at| span_text(now, *at, 0))
            .collect();
        if shown.is_empty() {
            "最近七天里没有场次".to_owned()
        } else if shown.len() > SHOWN_OCCURRENCES {
            format!(
                "{label}：{} 等 {} 场",
                shown[..SHOWN_OCCURRENCES].join("、"),
                shown.len()
            )
        } else {
            format!("{label}：{}", shown.join("、"))
        }
    };
    format!(
        "- {} [id {} v{}] {times}{until}{} · {}记下的",
        content.title,
        schedule.id,
        schedule.version,
        extras(content),
        schedule.created_by
    )
}

/// 一条提醒的样子。
pub fn reminder_line(now: Now, reminder: &Reminder) -> String {
    let due = reminder.content.due_at.as_millis();
    let state = if reminder.status == ReminderStatus::Fired {
        " · 已到点"
    } else if due <= now.now() {
        " · 已经过了到点时间"
    } else {
        ""
    };
    let notes = reminder
        .content
        .notes
        .as_ref()
        .map_or_else(String::new, |notes| format!(" · 备注：{notes}"));
    format!(
        "- {} [id {} v{}] {}{state}{notes} · {}记下的",
        reminder.content.title,
        reminder.id,
        reminder.version,
        stamp(due),
        reminder.created_by
    )
}

/// 一条没读的到点提醒的样子：它是哪一条日程或提醒的，带着什么话。
fn notification_line(notification: &Notification) -> String {
    let from = match (&notification.schedule_id, &notification.reminder_id) {
        (Some(id), _) => format!("来自日程 {id}"),
        (None, Some(id)) => format!("来自提醒 {id}"),
        (None, None) => "来源不详".to_owned(),
    };
    let start = notification.occurrence_at.map_or_else(String::new, |at| {
        format!("{} 开始 · ", stamp(at.as_millis()))
    });
    // 几句话里的换行压成一行：一条通知永远只占一行。
    let body = notification.body.as_ref().map_or_else(String::new, |body| {
        format!("{} · ", body.replace('\n', " / "))
    });
    format!(
        "- {} [通知 {}] · {start}{body}{} 生成 · {from}",
        notification.title,
        notification.id,
        stamp(notification.raised_at.as_millis()),
    )
}

/// 整份名单，写给助手看。
pub fn planner_text(
    planner: &Planner,
    account: &Account,
    device: &Device,
    now: Now,
    days: u32,
) -> String {
    let today = now.today_start();
    // 看到那一天的最后一刻为止（不含）。
    let until = now.days_ahead(days);
    let mut lines = Vec::new();

    lines.push(format!(
        "现在是 {}，时区 {}。下面的时间都是这台电脑的本地时间。",
        stamp(now.now()),
        now.offset()
    ));
    lines.push(format!(
        "账户：{}（{}）。你在账户里是一台叫“{}”的设备。",
        account.display_name, account.username, device.name
    ));
    lines.push(String::new());

    lines.push("【还没读的到点提醒】".to_owned());
    if planner.notifications.is_empty() {
        lines.push("（没有）".to_owned());
    }
    for notification in &planner.notifications {
        lines.push(notification_line(notification));
    }
    lines.push(String::new());

    lines.push(format!(
        "【{} 起的安排，到 {}】",
        date_part(today),
        date_part(until - 1)
    ));
    let (live, finished): (Vec<&Schedule>, Vec<&Schedule>) =
        planner.schedules.iter().partition(|schedule| {
            schedule.status == assemble_protocol::schedule::ScheduleStatus::Upcoming
        });
    // 这几天里一场都没有的日程（早就停了的重复、下周才有的一次性安排）不占地方。
    let mut upcoming: Vec<&Schedule> = live
        .iter()
        .copied()
        .filter(|schedule| !occurrences_of(schedule, today, until).is_empty())
        .collect();
    upcoming.sort_by_key(|schedule| {
        occurrences_of(schedule, today, until)
            .first()
            .copied()
            .unwrap_or(i64::MAX)
    });
    lines.push("日程：".to_owned());
    if upcoming.is_empty() {
        lines.push("（没有）".to_owned());
    }
    for schedule in &upcoming {
        lines.push(schedule_line(now, schedule, until));
    }
    lines.push("提醒：".to_owned());
    let reminders: Vec<&Reminder> = planner
        .reminders
        .iter()
        .filter(|reminder| {
            matches!(
                reminder.status,
                ReminderStatus::Scheduled | ReminderStatus::Fired
            )
        })
        .collect();
    if reminders.is_empty() {
        lines.push("（没有）".to_owned());
    }
    for reminder in reminders {
        lines.push(reminder_line(now, reminder));
    }
    if !finished.is_empty() {
        lines.push(String::new());
        lines.push("【最近完成或取消的日程，还能改回进行中】".to_owned());
        for schedule in &finished {
            let state = if schedule.status == assemble_protocol::schedule::ScheduleStatus::Done {
                "已完成"
            } else {
                "已取消"
            };
            lines.push(format!(
                "- {} [id {} v{}] {state} · {}记下的",
                schedule.content.title, schedule.id, schedule.version, schedule.created_by
            ));
        }
    }
    lines.push(String::new());
    lines.push("要改或取消哪一条，带上它的 id 和 v（版本号）。".to_owned());
    lines.join("\n")
}

/// 新建或修改之后回给助手的那一句：存成了什么样。只报最近七天，和默认的名单看齐。
pub fn saved_text(now: Now, schedule: &Schedule) -> String {
    format!(
        "已保存：{}",
        schedule_line(now, schedule, now.days_ahead(7)).trim_start_matches("- ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .unwrap()
    }

    fn at(local: DateTime<Local>) -> Timestamp {
        Timestamp::from_millis(local.timestamp_millis()).unwrap()
    }

    /// 2026-11-25 是周三。
    fn now() -> Now {
        Now::at(local(2026, 11, 25, 14, 0))
    }

    fn content(title: &str, start: DateTime<Local>, hours: Option<i64>) -> ScheduleContent {
        let start_at = at(start);
        ScheduleContent {
            title: title.into(),
            location: None,
            start_at,
            end_at: hours.map(|hours| at(start + chrono::Duration::hours(hours))),
            repeat: Repeat::None,
            repeat_until: None,
            remind_before: None,
            notes: None,
        }
    }

    fn schedule(
        id: &str,
        title: &str,
        start: DateTime<Local>,
        hours: Option<i64>,
        repeat: Repeat,
    ) -> Schedule {
        Schedule {
            id: id.into(),
            version: 3,
            status: assemble_protocol::schedule::ScheduleStatus::Upcoming,
            created_by: "ZCode 的助手".into(),
            created_at: at(start),
            updated_at: at(start),
            content: ScheduleContent {
                repeat,
                ..content(title, start, hours)
            },
        }
    }

    #[test]
    fn times_need_a_zone_and_come_back_as_utc() {
        // 同一个时刻的两种写法。
        let with_offset = parse_time("2026-11-26T09:00:00+08:00").unwrap();
        assert_eq!(with_offset, parse_time(" 2026-11-26T01:00:00Z ").unwrap());
        assert_eq!(
            with_offset,
            parse_time("2026-11-25T18:00:00-07:00").unwrap()
        );
        assert_eq!(
            parse_time("2026-11-26T01:00:00.500Z").unwrap().as_millis() % 1000,
            500
        );
        assert_eq!(
            parse_time("2026-11-26T09:00:00"),
            Err(TimeProblem::NoTimeZone)
        );
        assert_eq!(parse_time("2026-11-26T09:00"), Err(TimeProblem::NoTimeZone));
        assert_eq!(parse_time("明天九点"), Err(TimeProblem::NotADateTime));
        assert_eq!(
            parse_time("2026-13-45T09:00:00+08:00"),
            Err(TimeProblem::NotADateTime)
        );
        assert!(parse_time("9999-12-31T23:59:59Z").is_ok());
    }

    #[test]
    fn messages_tell_the_assistant_what_is_wrong() {
        assert!(
            TimeProblem::NoTimeZone
                .message("开始时间")
                .contains("开始时间没有带时区")
        );
        assert!(
            TimeProblem::NotADateTime
                .message("结束时间")
                .contains("+08:00")
        );
        assert!(TimeProblem::OutOfRange.message("到点时间").contains("年份"));
    }

    fn who(now: Now) -> (Account, Device) {
        (
            Account {
                id: "a".into(),
                username: "alex".into(),
                display_name: "Alex".into(),
                email: None,
                avatar_url: None,
                created_at: at(now.local),
            },
            Device {
                id: "d".into(),
                name: "ZCode 的助手".into(),
                platform: assemble_protocol::primitives::Platform::Macos,
                online: false,
                last_seen_at: None,
                created_at: at(now.local),
            },
        )
    }

    #[test]
    fn the_list_names_full_dates_with_the_weekday_and_carries_ids_and_versions() {
        let now = now();
        let mut done = schedule(
            "s-done",
            "旧的事",
            local(2026, 11, 24, 9, 0),
            Some(1),
            Repeat::None,
        );
        done.status = assemble_protocol::schedule::ScheduleStatus::Done;
        let planner = Planner {
            navigation: Vec::new(),
            notifications: vec![
                assemble_protocol::schedule::Notification {
                    id: "note-1".into(),
                    kind: assemble_protocol::schedule::NotificationKind::Reminder,
                    title: "取快递".into(),
                    body: Some("驿站楼下\n记得带小票".into()),
                    schedule_id: None,
                    reminder_id: Some("reminder-1".into()),
                    occurrence_at: None,
                    raised_at: at(local(2026, 11, 25, 9, 0)),
                },
                assemble_protocol::schedule::Notification {
                    id: "note-2".into(),
                    kind: assemble_protocol::schedule::NotificationKind::Schedule,
                    title: "周会".into(),
                    body: Some("三楼小会议室".into()),
                    schedule_id: Some("s-daily".into()),
                    reminder_id: None,
                    occurrence_at: Some(at(local(2026, 11, 25, 9, 0))),
                    raised_at: at(local(2026, 11, 25, 8, 30)),
                },
            ],
            schedules: vec![
                schedule(
                    "s-daily",
                    "周会",
                    local(2026, 11, 25, 9, 0),
                    Some(1),
                    Repeat::Daily,
                ),
                schedule(
                    "s-train",
                    "火车",
                    local(2026, 11, 26, 9, 0),
                    Some(1),
                    Repeat::None,
                ),
                done,
                // 八天之后才有：这七天里排不进来。
                schedule(
                    "s-later",
                    "下下周的事",
                    local(2026, 12, 3, 9, 0),
                    None,
                    Repeat::None,
                ),
            ],
            reminders: vec![Reminder {
                id: "reminder-1".into(),
                version: 2,
                status: ReminderStatus::Fired,
                created_by: "本机".into(),
                created_at: at(now.local),
                updated_at: at(now.local),
                content: assemble_protocol::schedule::ReminderContent {
                    title: "取快递".into(),
                    due_at: at(local(2026, 11, 25, 9, 0)),
                    notes: Some("驿站楼下".into()),
                },
            }],
        };
        let (account, device) = who(now);

        let text = planner_text(&planner, &account, &device, now, 7);
        let line = |needle: &str| {
            text.lines()
                .find(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("{needle} is missing from:\n{text}"))
                .to_owned()
        };

        assert!(text.starts_with("现在是 2026-11-25 周三 14:00，时区 "));
        assert!(text.contains("账户：Alex（alex）。你在账户里是一台叫“ZCode 的助手”的设备。"));
        assert_eq!(
            line(" 起的安排").strip_prefix("【").unwrap(),
            "2026-11-25 周三 起的安排，到 2026-12-01 周二】"
        );
        assert_eq!(
            line("note-1"),
            "- 取快递 [通知 note-1] · 驿站楼下 / 记得带小票 · 2026-11-25 周三 09:00 生成 · 来自提醒 reminder-1"
        );
        assert_eq!(
            line("note-2"),
            "- 周会 [通知 note-2] · 2026-11-25 周三 09:00 开始 · 三楼小会议室 · 2026-11-25 周三 08:30 生成 · 来自日程 s-daily"
        );
        // 天天有的：今天上午那一场已经过去，照样列出来并标上"已过"。
        assert_eq!(
            line("[id s-daily"),
            "- 周会 [id s-daily v3] 每天：2026-11-25 周三 09:00（已过）、2026-11-26 周四 09:00、2026-11-27 周五 09:00 等 7 场 · ZCode 的助手记下的"
        );
        assert_eq!(
            line("s-train"),
            "- 火车 [id s-train v3] 2026-11-26 周四 09:00 到 10:00 · ZCode 的助手记下的"
        );
        assert!(!text.contains("s-later"), "{text}");
        assert!(text.contains("【最近完成或取消的日程，还能改回进行中】"));
        assert_eq!(
            line("[id s-done"),
            "- 旧的事 [id s-done v3] 已完成 · ZCode 的助手记下的"
        );
        assert_eq!(
            line("[id reminder-1"),
            "- 取快递 [id reminder-1 v2] 2026-11-25 周三 09:00 · 已到点 · 备注：驿站楼下 · 本机记下的"
        );
        // 日程按最近的一场排：今天有场的在前。
        let daily = text.find("[id s-daily").unwrap();
        let train = text.find("[id s-train").unwrap();
        assert!(daily < train);
        assert!(text.ends_with("要改或取消哪一条，带上它的 id 和 v（版本号）。"));
    }

    #[test]
    fn the_window_follows_the_number_of_days_asked_for() {
        let now = now();
        let (account, device) = who(now);
        let planner = Planner {
            navigation: Vec::new(),
            notifications: Vec::new(),
            schedules: vec![schedule(
                "s-later",
                "下下周的事",
                local(2026, 12, 3, 9, 0),
                None,
                Repeat::None,
            )],
            reminders: Vec::new(),
        };
        assert!(!planner_text(&planner, &account, &device, now, 7).contains("s-later"));
        let wider = planner_text(&planner, &account, &device, now, 14);
        assert!(wider.contains("【2026-11-25 周三 起的安排，到 2026-12-08 周二】"));
        assert!(wider.contains("s-later"));
    }

    #[test]
    fn a_schedule_with_everything_attached() {
        let now = now();
        let mut full = schedule(
            "s-full",
            "火车",
            local(2026, 11, 26, 23, 0),
            Some(2),
            Repeat::None,
        );
        full.content.location = Some(Location {
            label: "上海虹桥站".into(),
            address: "申虹路 1500 号".into(),
        });
        full.content.remind_before = Some(90);
        full.content.notes = Some("带身份证".into());
        assert_eq!(
            schedule_line(now, &full, now.days_ahead(7)),
            "- 火车 [id s-full v3] 2026-11-26 周四 23:00 到 2026-11-27 周五 01:00 · 地点：上海虹桥站 · 提前 90 分钟提醒 · 备注：带身份证 · ZCode 的助手记下的"
        );
        assert!(saved_text(now, &full).starts_with("已保存：火车 [id s-full v3]"));
    }

    #[test]
    fn overlapping_and_back_to_back_are_told_apart() {
        let now = now();
        let meeting = schedule(
            "s-meeting",
            "周会",
            local(2026, 11, 26, 10, 0),
            Some(2),
            Repeat::None,
        );
        let nine_to_ten = content("火车", local(2026, 11, 26, 9, 0), Some(1));
        // 九点到十点，紧接着十点的周会：不算撞车。
        assert!(conflicts(&nine_to_ten, std::slice::from_ref(&meeting), now, None).is_empty());

        let into_it = content("火车", local(2026, 11, 26, 9, 0), Some(2));
        assert_eq!(
            conflicts(&into_it, std::slice::from_ref(&meeting), now, None),
            vec![Conflict {
                title: "周会".into(),
                at_ms: local(2026, 11, 26, 10, 0).timestamp_millis(),
            }]
        );

        // 没写结束时间的，落在别人的时段里才算；刚好在周会结束的那一刻不算。
        let inside = content("顺手的事", local(2026, 11, 26, 11, 0), None);
        assert_eq!(
            conflicts(&inside, std::slice::from_ref(&meeting), now, None).len(),
            1
        );
        let at_the_end = content("卡在结尾", local(2026, 11, 26, 12, 0), None);
        assert!(conflicts(&at_the_end, std::slice::from_ref(&meeting), now, None).is_empty());
    }

    #[test]
    fn editing_a_schedule_does_not_compare_it_with_itself() {
        let now = now();
        let whole = schedule(
            "s-meeting",
            "周会",
            local(2026, 11, 26, 10, 0),
            Some(2),
            Repeat::None,
        );
        assert!(
            conflicts(
                &whole.content,
                std::slice::from_ref(&whole),
                now,
                Some("s-meeting")
            )
            .is_empty()
        );
        assert_eq!(
            conflicts(
                &whole.content,
                std::slice::from_ref(&whole),
                now,
                Some("s-other")
            )
            .len(),
            1
        );
    }

    #[test]
    fn a_repeating_schedule_is_checked_at_each_occurrence() {
        let now = now();
        let daily = schedule(
            "s-daily",
            "每天站会",
            local(2026, 11, 20, 9, 0),
            Some(1),
            Repeat::Daily,
        );
        let dentist = content("看牙", local(2026, 11, 27, 9, 30), Some(1));
        assert_eq!(
            conflicts(&dentist, std::slice::from_ref(&daily), now, None),
            vec![Conflict {
                title: "每天站会".into(),
                at_ms: local(2026, 11, 27, 9, 0).timestamp_millis(),
            }]
        );
        // 四周之后的事也照样比：那天 9 点的站会还没开完，9 点半的牙医和它撞上。
        let later = content("看牙", local(2026, 12, 23, 9, 30), Some(1));
        assert_eq!(
            conflicts(&later, std::slice::from_ref(&daily), now, None),
            vec![Conflict {
                title: "每天站会".into(),
                at_ms: local(2026, 12, 23, 9, 0).timestamp_millis(),
            }]
        );
        // 等站会开完再去：不撞。
        let after = content("看牙", local(2026, 12, 23, 11, 0), Some(1));
        assert!(conflicts(&after, std::slice::from_ref(&daily), now, None).is_empty());
    }

    #[test]
    fn conflict_hints_are_capped_and_ordered_by_time() {
        let now = now();
        let others: Vec<Schedule> = (0..8)
            .map(|index| {
                schedule(
                    &format!("s-{index}"),
                    &format!("第 {index} 件"),
                    local(2026, 11, 26, 9, 0) + chrono::Duration::minutes(index),
                    Some(30),
                    Repeat::None,
                )
            })
            .collect();
        let found = conflicts(
            &content("大事", local(2026, 11, 26, 9, 0), Some(60)),
            &others,
            now,
            None,
        );
        assert_eq!(found.len(), 8);
        assert!(found.windows(2).all(|pair| pair[0].at_ms <= pair[1].at_ms));
        assert_eq!(
            conflict_text(&found),
            "提示：它和「第 0 件」、「第 1 件」、「第 2 件」、「第 3 件」、「第 4 件」 等 8 场安排撞车。"
        );

        let one = conflicts(
            &content("火车", local(2026, 11, 26, 10, 30), Some(1)),
            std::slice::from_ref(&others[0]),
            now,
            None,
        );
        assert_eq!(
            conflict_text(&one),
            "提示：它和「第 0 件」（2026-11-26 周四 09:00）撞车。"
        );
        assert_eq!(conflict_text(&[]), "");
    }
}
