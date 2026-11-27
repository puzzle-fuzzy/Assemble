//! 日程规划的第一批：日程、提醒，以及它们到点之后生成的通知。
//!
//! 日程是一段有开始时间的安排，可以按天、周、月重复；提醒是一件到点要做的事。
//! 两者到点后各生成一条通知，推给这个账户的每一台在线设备。
//!
//! 这里只放类型与纯校验。什么时候该提醒、每一场只提醒一次，是服务端的事
//! （`apps/server`）；重复规则怎么推算，在 `assemble-schedule` 里，两端共用。

use serde::{Deserialize, Serialize};

use crate::primitives::{Timestamp, is_text};

/// 标题的上限（UTF-16 单元）。
pub const TITLE_MAX_UNITS: usize = 200;
/// 备注的上限（UTF-16 单元）。
pub const NOTES_MAX_UNITS: usize = 4000;
/// 地点的名字与导航地址的上限（UTF-16 单元）。
pub const LOCATION_LABEL_MAX_UNITS: usize = 200;
pub const LOCATION_ADDRESS_MAX_UNITS: usize = 500;
/// 最多提前多少分钟提醒：一天。
pub const REMIND_BEFORE_MAX_MINUTES: u32 = 1440;
/// 一条日程最长跨多久：一年。
pub const LONGEST_EVENT_SECONDS: i64 = 366 * 24 * 60 * 60;

/// 时间的下限：1990-01-01T00:00:00Z 的毫秒数。再早的日程多半是把年份填错了。
pub const EARLIEST_MILLIS: i64 = 631_152_000_000;
/// 时间的上限：9999-12-31T23:59:59Z 的毫秒数。月重复要在这之上做日历运算，不能贴着 i64 的边。
pub const LATEST_MILLIS: i64 = 253_402_300_799_000;

/// 一条日程的重复规则。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    #[default]
    None,
    Daily,
    Weekly,
    Monthly,
}

/// 一条日程的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleStatus {
    Upcoming,
    Done,
    Cancelled,
}

/// 一条提醒的状态。`fired` 只由服务端在到点时写入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReminderStatus {
    Scheduled,
    Fired,
    Dismissed,
    Cancelled,
}

/// 一处地点：`label` 是给人看的地名，`address` 是拿去导航的地址。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub label: String,
    pub address: String,
}

impl Location {
    /// 与 Jarvis 一致的公共地图链接；地址只能成为编码后的单个参数。
    pub fn directions_url(&self) -> Option<url::Url> {
        if self.address.trim().is_empty()
            || self.address.encode_utf16().count() > 1000
            || self.address.chars().any(char::is_control)
        {
            return None;
        }
        let mut url = url::Url::parse("https://maps.apple.com/directions").ok()?;
        url.query_pairs_mut()
            .append_pair("destination", self.address.trim());
        Some(url)
    }
}

/// 只读地点绑定；来源名称固定为 schedule、reminder、memo。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordLocations {
    pub resource: String,
    pub id: String,
    pub locations: Vec<Location>,
}

/// 一条日程可以改动的全部内容。新建时整份给出，修改时也整份给出：
/// 修改带着版本号，整份替换就不会因为两边各改一半而互相覆盖。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleContent {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    pub start_at: Timestamp,
    /// 结束时间。重复日程的每一场都持续同样长。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_at: Option<Timestamp>,
    #[serde(default)]
    pub repeat: Repeat,
    /// 重复到什么时候为止（含）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_until: Option<Timestamp>,
    /// 提前多少分钟提醒；不填或为零就是到点才提醒。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remind_before: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl ScheduleContent {
    /// 提前多少分钟提醒；零与没填是一样的。
    pub fn lead_minutes(&self) -> u32 {
        self.remind_before.unwrap_or(0)
    }

    /// 两端的空白去掉、零提前量当成没填之后的样子。哪一项不合规则就返回 `None`。
    pub fn cleaned(&self) -> Option<Self> {
        let title = single_line(&self.title, TITLE_MAX_UNITS)?;
        let location = match &self.location {
            Some(location) => {
                let label = single_line(&location.label, LOCATION_LABEL_MAX_UNITS)?;
                let address = single_line(&location.address, LOCATION_ADDRESS_MAX_UNITS)?;
                Some(Location {
                    label: label.to_owned(),
                    address: address.to_owned(),
                })
            }
            None => None,
        };
        if !in_range(self.start_at) {
            return None;
        }
        let end_at = match self.end_at {
            Some(end_at) => {
                if !in_range(end_at) {
                    return None;
                }
                let millis = end_at.as_millis() - self.start_at.as_millis();
                // 结束必须晚于开始；也不能长到像把年份填错了。
                if millis <= 0 || millis > LONGEST_EVENT_SECONDS * 1000 {
                    return None;
                }
                Some(end_at)
            }
            None => None,
        };
        let repeat_until = match (self.repeat, self.repeat_until) {
            (_, Some(until)) => {
                if !in_range(until) || until < self.start_at {
                    return None;
                }
                Some(until)
            }
            (_, None) => None,
        };
        let remind_before = match self.lead_minutes() {
            0 => None,
            minutes if minutes <= REMIND_BEFORE_MAX_MINUTES => Some(minutes),
            _ => return None,
        };
        // 只有空白的备注等于没有备注。
        let notes = match &self.notes {
            Some(notes) if notes.trim().is_empty() => None,
            Some(notes) => Some(multiline(notes, NOTES_MAX_UNITS)?.to_owned()),
            None => None,
        };
        Some(Self {
            title: title.to_owned(),
            location,
            start_at: self.start_at,
            end_at,
            repeat: self.repeat,
            repeat_until,
            remind_before,
            notes,
        })
    }
}

/// 一条日程在账户里的样子。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub id: String,
    /// 每改一次加一。修改时要带上看到的那个版本，对不上就被拒绝。
    pub version: i64,
    pub status: ScheduleStatus,
    /// 建立它的那台设备当时叫什么：助手写的和手建的，在界面上分得开。
    pub created_by: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(flatten)]
    pub content: ScheduleContent,
}

/// 修改一条日程：带上看到的版本号，整份替换内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleUpdate {
    pub version: i64,
    #[serde(flatten)]
    pub content: ScheduleContent,
}

/// 一条提醒可以改动的全部内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderContent {
    pub title: String,
    pub due_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl ReminderContent {
    pub fn cleaned(&self) -> Option<Self> {
        let title = single_line(&self.title, TITLE_MAX_UNITS)?;
        if !in_range(self.due_at) {
            return None;
        }
        // 只有空白的备注等于没有备注。
        let notes = match &self.notes {
            Some(notes) if notes.trim().is_empty() => None,
            Some(notes) => Some(multiline(notes, NOTES_MAX_UNITS)?.to_owned()),
            None => None,
        };
        Some(Self {
            title: title.to_owned(),
            due_at: self.due_at,
            notes,
        })
    }
}

/// 一条提醒在账户里的样子。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reminder {
    pub id: String,
    pub version: i64,
    pub status: ReminderStatus,
    pub created_by: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(flatten)]
    pub content: ReminderContent,
}

/// 修改一条提醒：带上看到的版本号，整份替换内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderUpdate {
    pub version: i64,
    #[serde(flatten)]
    pub content: ReminderContent,
}

/// 改状态、删掉：都要带上看到的版本号。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Versioned {
    pub version: i64,
}

/// 通知来自一条日程还是一条提醒。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationKind {
    Schedule,
    Reminder,
}

/// 到点之后生成的一条通知。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: String,
    pub kind: NotificationKind,
    pub title: String,
    /// 给人看的几句话：备注、地点，或者这一场几点开始。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reminder_id: Option<String>,
    /// 日程的哪一场；提醒没有这一项。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrence_at: Option<Timestamp>,
    pub raised_at: Timestamp,
}

/// `GET /v1/planner`：一个账户的日程、提醒与还没读的通知。
///
/// 名单只含还在进行中的，加上最近七天里结束的（免得刚标完成的那一条当场消失）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Planner {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub navigation: Vec<RecordLocations>,
    pub schedules: Vec<Schedule>,
    pub reminders: Vec<Reminder>,
    /// 还没读的通知，新的在前。
    pub notifications: Vec<Notification>,
}

/// 把几条通知标成已读。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadInput {
    pub ids: Vec<String>,
}

fn single_line(value: &str, max_units: usize) -> Option<&str> {
    let trimmed = value.trim();
    is_text(trimmed, max_units)
        .then_some(trimmed)
        .filter(|_| !trimmed.chars().any(char::is_control))
}

fn multiline(value: &str, max_units: usize) -> Option<&str> {
    let trimmed = value.trim();
    is_text(trimmed, max_units).then_some(trimmed).filter(|_| {
        trimmed
            .chars()
            .all(|character| !character.is_control() || matches!(character, '\n' | '\t'))
    })
}

fn in_range(at: Timestamp) -> bool {
    (EARLIEST_MILLIS..=LATEST_MILLIS).contains(&at.as_millis())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn map_destination_cannot_escape_the_fixed_origin_or_inject_parameters() {
        let location = Location {
            label: "车站".into(),
            address: "上海&x=javascript:alert(1)#片段".into(),
        };
        let url = location.directions_url().unwrap();
        assert_eq!(url.origin().ascii_serialization(), "https://maps.apple.com");
        assert_eq!(url.query_pairs().count(), 1);
        assert_eq!(url.query_pairs().next().unwrap().1, location.address);
        assert!(
            Location {
                address: "bad\naddress".into(),
                ..location
            }
            .directions_url()
            .is_none()
        );
    }

    fn at(text: &str) -> Timestamp {
        serde_json::from_value(json!(text)).unwrap()
    }

    fn content() -> ScheduleContent {
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

    #[test]
    fn titles_and_notes_are_trimmed_and_bounded() {
        let cleaned = content().cleaned().unwrap();
        assert_eq!(cleaned.title, "火车");

        let mut blank = content();
        blank.title = "   ".into();
        assert!(blank.cleaned().is_none());
        let mut too_long = content();
        too_long.title = "名".repeat(TITLE_MAX_UNITS + 1);
        assert!(too_long.cleaned().is_none());
        let mut control = content();
        control.title = "a\nb".into();
        assert!(control.cleaned().is_none());

        let mut notes = content();
        notes.notes = Some("  带身份证\n  别迟到\t".into());
        assert_eq!(
            notes.cleaned().unwrap().notes.as_deref(),
            Some("带身份证\n  别迟到")
        );
        notes.notes = Some("a\u{7}b".into());
        assert!(notes.cleaned().is_none());
        notes.notes = Some(" ".into());
        // 只有空白的备注等于没有备注。
        assert_eq!(notes.cleaned().unwrap().notes, None);
    }

    #[test]
    fn times_must_be_ordered_and_within_a_sane_range() {
        assert!(content().cleaned().is_some());

        let mut ends_before_it_starts = content();
        ends_before_it_starts.end_at = Some(at("2026-11-25T00:59:59.000Z"));
        assert!(ends_before_it_starts.cleaned().is_none());
        let mut same_moment = content();
        same_moment.end_at = Some(at("2026-11-25T01:00:00.000Z"));
        assert!(same_moment.cleaned().is_none());
        let mut a_year = content();
        a_year.end_at = Some(at("2027-11-25T01:00:00.000Z"));
        assert!(a_year.cleaned().is_some());

        let mut repeat_ends_too_early = content();
        repeat_ends_too_early.repeat = Repeat::Daily;
        repeat_ends_too_early.repeat_until = Some(at("2026-11-24T01:00:00.000Z"));
        assert!(repeat_ends_too_early.cleaned().is_none());
        let mut repeating = content();
        repeating.repeat = Repeat::Monthly;
        repeating.repeat_until = Some(at("2027-01-01T00:00:00.000Z"));
        assert_eq!(repeating.cleaned().unwrap().repeat, Repeat::Monthly);

        let mut far_past = content();
        far_past.start_at = at("1899-01-01T00:00:00.000Z");
        assert!(far_past.cleaned().is_none());

        let reminder = ReminderContent {
            title: "取快递".into(),
            due_at: at("2026-11-25T01:00:00.000Z"),
            notes: None,
        };
        assert!(reminder.cleaned().is_some());
        let mut far_future = reminder;
        far_future.due_at = at("9999-12-31T23:59:59.000Z");
        assert!(far_future.cleaned().is_some());
        far_future.due_at = Timestamp::from_millis(LATEST_MILLIS + 1000).unwrap();
        assert!(far_future.cleaned().is_none());
    }

    #[test]
    fn the_time_bounds_are_the_moments_the_comments_name() {
        let format = "%Y-%m-%dT%H:%M:%SZ";
        assert_eq!(
            chrono::DateTime::from_timestamp_millis(EARLIEST_MILLIS)
                .unwrap()
                .format(format)
                .to_string(),
            "1990-01-01T00:00:00Z"
        );
        assert_eq!(
            chrono::DateTime::from_timestamp_millis(LATEST_MILLIS)
                .unwrap()
                .format(format)
                .to_string(),
            "9999-12-31T23:59:59Z"
        );
    }

    #[test]
    fn a_zero_lead_is_the_same_as_none() {
        let mut zero = content();
        zero.remind_before = Some(0);
        assert_eq!(zero.cleaned().unwrap().remind_before, None);
        assert_eq!(zero.cleaned().unwrap().lead_minutes(), 0);
        let mut an_hour = content();
        an_hour.remind_before = Some(60);
        assert_eq!(
            an_hour.cleaned().unwrap().remind_before,
            Some(REMIND_BEFORE_MAX_MINUTES / 24)
        );
        let mut too_much = content();
        too_much.remind_before = Some(REMIND_BEFORE_MAX_MINUTES + 1);
        assert!(too_much.cleaned().is_none());
    }

    #[test]
    fn a_location_needs_a_label_and_an_address() {
        let mut place = content();
        place.location = Some(Location {
            label: " 上海虹桥站 ".into(),
            address: "上海市闵行区申虹路".into(),
        });
        assert_eq!(
            place.cleaned().unwrap().location.map(|l| l.label),
            Some("上海虹桥站".into())
        );
        place.location = Some(Location {
            label: " ".into(),
            address: "x".into(),
        });
        assert!(place.cleaned().is_none());
    }

    #[test]
    fn records_read_and_write_their_content_at_the_top_level() {
        let schedule: Schedule = serde_json::from_value(json!({
            "id": "s1",
            "version": 3,
            "status": "upcoming",
            "createdBy": "ZCode 的助手",
            "createdAt": "2026-11-24T12:00:00.000Z",
            "updatedAt": "2026-11-24T12:30:00.000Z",
            "title": "火车",
            "startAt": "2026-11-25T01:00:00.000Z",
            "repeat": "daily",
            "remindBefore": 30,
            "notes": "带身份证"
        }))
        .unwrap();
        assert_eq!(schedule.content.title, "火车");
        assert_eq!(schedule.content.repeat, Repeat::Daily);
        assert_eq!(schedule.content.remind_before, Some(30));
        assert!(schedule.content.end_at.is_none());
        assert_eq!(schedule.version, 3);
        // 写出去时内容还在最上面一层，空的可选字段不出现。
        let encoded = serde_json::to_value(&schedule).unwrap();
        assert_eq!(encoded["startAt"], "2026-11-25T01:00:00.000Z");
        assert!(encoded.get("endAt").is_none());
        assert!(encoded.get("repeatUntil").is_none());
        assert_eq!(encoded["status"], "upcoming");

        let update: ScheduleUpdate = serde_json::from_value(json!({
            "version": 3, "title": "火车", "startAt": "2026-11-25T01:00:00.000Z"
        }))
        .unwrap();
        assert_eq!(update.content.repeat, Repeat::None);
        assert_eq!(update.version, 3);

        let reminder: Reminder = serde_json::from_value(json!({
            "id": "r1", "version": 1, "status": "fired", "createdBy": "d",
            "createdAt": "2026-11-24T12:00:00.000Z", "updatedAt": "2026-11-24T12:00:00.000Z",
            "title": "取快递", "dueAt": "2026-11-25T01:00:00.000Z"
        }))
        .unwrap();
        assert_eq!(reminder.status, ReminderStatus::Fired);
        assert_eq!(reminder.content.due_at, at("2026-11-25T01:00:00.000Z"));

        let notification: Notification = serde_json::from_value(json!({
            "id": "s1:1", "kind": "schedule", "title": "火车",
            "scheduleId": "s1", "occurrenceAt": "2026-11-25T01:00:00.000Z",
            "raisedAt": "2026-11-25T00:30:00.000Z"
        }))
        .unwrap();
        assert_eq!(notification.kind, NotificationKind::Schedule);
        assert_eq!(notification.body, None);
        assert!(
            serde_json::to_value(&notification)
                .unwrap()
                .get("body")
                .is_none()
        );
    }
}
