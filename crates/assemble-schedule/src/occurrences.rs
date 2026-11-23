//! 日程出现规则的推算。
//!
//! 算法移植自 jarvis 的 `schedule-occurrences.ts`，并用同一批两万条用例对照过，
//! 结果逐项一致；月重复的钳制规则不要轻易改动，否则界面显示的时间会和
//! 实际提醒的时间对不上。
//!
//! 时间一律用毫秒时间戳。月重复遵循固定的北京时间业务日历（UTC+08:00，
//! 不随夏令时变化）：先把时间戳整体平移八小时，在"平移后的日历"上做月份运算，
//! 再平移回来。

use crate::model::{Repeat, ScheduleData};

/// 一天的毫秒数。
pub const DAY_MS: i64 = 86_400_000;

/// 一周的毫秒数。
pub const WEEK_MS: i64 = 7 * DAY_MS;

/// 北京时间相对 UTC 的偏移。
pub const SHANGHAI_OFFSET_MS: i64 = 8 * 3_600_000;

/// 月重复最多向前推算的步数：一千年。
pub const MAX_STEPS: i64 = 12_000;

/// 一条日程的重复规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recurrence {
    /// 第一次出现的时刻。
    pub start_at: i64,
    pub repeat: Repeat,
    /// 重复截止时刻（含）。
    pub repeat_until: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RecurrenceError {
    #[error("the schedule has no valid start time")]
    InvalidStart,
    #[error("the schedule has a repeat end that is not a valid time")]
    InvalidRepeatUntil,
}

impl Recurrence {
    /// 从一条日程记录读出规则。开始时间无效的日程永远不会出现。
    pub fn from_schedule(schedule: &ScheduleData) -> Result<Self, RecurrenceError> {
        let start_at = parse(&schedule.start_at).ok_or(RecurrenceError::InvalidStart)?;
        let repeat_until = match &schedule.repeat_until {
            Some(value) => Some(parse(value).ok_or(RecurrenceError::InvalidRepeatUntil)?),
            None => None,
        };
        Ok(Self {
            start_at,
            repeat: schedule.repeat,
            repeat_until,
        })
    }

    /// 不晚于 `now_ms` 的最近一次出现。
    ///
    /// 日程还没开始、重复窗口已经关闭时返回 `None`。只有最近这一次有意义：
    /// 过去的出现不会被补发。
    pub fn latest_at_or_before(&self, now_ms: i64) -> Option<i64> {
        if now_ms < self.start_at {
            return None;
        }
        let occurrence = at_ordinal(
            self.start_at,
            self.repeat,
            ordinal_at(self.start_at, self.repeat, now_ms),
        );
        if self.repeat_until.is_some_and(|until| occurrence > until) {
            return None;
        }
        Some(occurrence)
    }

    /// 从 `from_ms` 起接下来的几次出现（含 `from_ms` 当时刻），最多 `limit` 次。
    pub fn upcoming(&self, from_ms: i64, limit: usize) -> Vec<i64> {
        if limit == 0 {
            return Vec::new();
        }
        // `from_ms` 前一刻所在的序号加一，就是第一个不早于它的出现。
        let first = ordinal_at(self.start_at, self.repeat, from_ms.saturating_sub(1)) + 1;
        let first = first.max(0);
        let mut upcoming = Vec::new();
        for ordinal in first.. {
            if self.repeat == Repeat::None && ordinal > 0 {
                break;
            }
            let occurrence = at_ordinal(self.start_at, self.repeat, ordinal);
            if self.repeat_until.is_some_and(|until| occurrence > until) {
                break;
            }
            upcoming.push(occurrence);
            if upcoming.len() == limit {
                break;
            }
        }
        upcoming
    }
}

/// 一个闭区间内出现的次数与首末时刻，不需要逐个枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OccurrenceRange {
    pub first: i64,
    pub last: i64,
    pub count: i64,
}

/// 落在闭区间 `[lower_ms, upper_ms]` 内的出现。
pub fn occurrence_range(
    rule: &Recurrence,
    lower_ms: i64,
    upper_ms: i64,
) -> Option<OccurrenceRange> {
    let last_allowed = rule
        .repeat_until
        .map_or(upper_ms, |until| upper_ms.min(until));
    if last_allowed < rule.start_at || lower_ms > last_allowed {
        return None;
    }
    let first = ordinal_at(rule.start_at, rule.repeat, lower_ms.saturating_sub(1)) + 1;
    let last = ordinal_at(rule.start_at, rule.repeat, last_allowed);
    if last < first {
        return None;
    }
    Some(OccurrenceRange {
        first: at_ordinal(rule.start_at, rule.repeat, first),
        last: at_ordinal(rule.start_at, rule.repeat, last),
        count: last - first + 1,
    })
}

/// `at_ms` 所在的出现序号；早于第一次出现时为 -1。
fn ordinal_at(start: i64, repeat: Repeat, at_ms: i64) -> i64 {
    if at_ms < start {
        return -1;
    }
    match repeat {
        Repeat::Daily => (at_ms - start) / DAY_MS,
        Repeat::Weekly => (at_ms - start) / WEEK_MS,
        Repeat::Monthly => MAX_STEPS
            .min(months_between(start + SHANGHAI_OFFSET_MS, at_ms + SHANGHAI_OFFSET_MS).max(0)),
        Repeat::None => 0,
    }
}

/// 第 `ordinal` 次出现的时刻。
fn at_ordinal(start: i64, repeat: Repeat, ordinal: i64) -> i64 {
    match repeat {
        Repeat::Daily => start + ordinal * DAY_MS,
        Repeat::Weekly => start + ordinal * WEEK_MS,
        Repeat::Monthly => {
            month_occurrence(start + SHANGHAI_OFFSET_MS, ordinal) - SHANGHAI_OFFSET_MS
        }
        Repeat::None => start,
    }
}

/// 从 `start` 到 `current` 为止，最后一个不晚于 `current` 的月度序号；假定 `start <= current`。
fn months_between(start: i64, current: i64) -> i64 {
    let from = Civil::from_ms(start);
    let to = Civil::from_ms(current);
    let mut months = (to.year - from.year) * 12 + i64::from(to.month) - i64::from(from.month);
    if month_occurrence(start, months) > current {
        months -= 1;
    }
    months
}

/// `start` 之后第 `months` 个月的同一天；短月份钳制到当月最后一天。
fn month_occurrence(start: i64, months: i64) -> i64 {
    let date = Civil::from_ms(start);
    // 月份按"年 × 12 + 月"连续编号，跨年与负数都由除法自然处理。
    let total = date.year * 12 + i64::from(date.month) + months;
    let year = total.div_euclid(12);
    let month = u32::try_from(total.rem_euclid(12)).unwrap_or(0);
    let day = date.day.min(days_in_month(year, month));
    Civil { year, month, day }.to_ms() + start.rem_euclid(DAY_MS)
}

/// 某年某月的天数。`month` 从 0 开始，1 表示二月。
fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        0 | 2 | 4 | 6 | 7 | 9 | 11 => 31,
        3 | 5 | 8 | 10 => 30,
        1 if is_leap(year) => 29,
        1 => 28,
        _ => 0,
    }
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// 公历日期：`month` 从 0 开始，与云端的 JavaScript 实现一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Civil {
    year: i64,
    month: u32,
    day: u32,
}

impl Civil {
    /// 毫秒时间戳对应的日期。
    fn from_ms(ms: i64) -> Self {
        Self::from_days(ms.div_euclid(DAY_MS))
    }

    /// 当天零点的毫秒时间戳。
    fn to_ms(self) -> i64 {
        self.to_days() * DAY_MS
    }

    /// 自 1970-01-01 起的天数转日期（Howard Hinnant 的算法）。
    fn from_days(days: i64) -> Self {
        let shifted = days + 719_468;
        let era = if shifted >= 0 {
            shifted
        } else {
            shifted - 146_096
        } / 146_097;
        let day_of_era = shifted - era * 146_097;
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let year = year_of_era + era * 400;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_pointer = (5 * day_of_year + 2) / 153;
        let day =
            u32::try_from(day_of_year - (153 * month_pointer + 2) / 5 + 1).unwrap_or_default();
        let month = if month_pointer < 10 {
            month_pointer + 3
        } else {
            month_pointer - 9
        };
        Self {
            year: if month <= 2 { year + 1 } else { year },
            month: u32::try_from(month - 1).unwrap_or_default(),
            day,
        }
    }

    /// 日期转自 1970-01-01 起的天数。
    fn to_days(self) -> i64 {
        let year = if self.month <= 1 {
            self.year - 1
        } else {
            self.year
        };
        let era = if year >= 0 { year } else { year - 399 } / 400;
        let year_of_era = year - era * 400;
        let month = i64::from(self.month) + 1;
        let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5
            + i64::from(self.day)
            - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }
}

/// 解析带时区偏移的 ISO 时间。
fn parse(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|instant| instant.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, TimeZone};
    use serde_json::json;

    use super::*;
    use crate::model::ScheduleStatus;

    /// 北京时间某年某月某日某时对应的毫秒时间戳。用 chrono 独立算出期望值，
    /// 不依赖被测代码里的日历换算。
    fn beijing(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        FixedOffset::east_opt(8 * 3600)
            .unwrap()
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
            .timestamp_millis()
    }

    fn iso(ms: i64) -> String {
        chrono::DateTime::from_timestamp_millis(ms)
            .unwrap()
            .to_rfc3339()
    }

    fn schedule(start: i64, repeat: Repeat, repeat_until: Option<i64>) -> ScheduleData {
        ScheduleData {
            title: "周会".into(),
            location: None,
            start_at: iso(start),
            end_at: None,
            repeat,
            repeat_until: repeat_until.map(iso),
            status: ScheduleStatus::Upcoming,
            notes: None,
        }
    }

    fn rule(start: i64, repeat: Repeat, repeat_until: Option<i64>) -> Recurrence {
        Recurrence::from_schedule(&schedule(start, repeat, repeat_until)).unwrap()
    }

    #[test]
    fn a_one_time_schedule_occurs_exactly_once() {
        let start = beijing(2026, 11, 24, 9, 0);
        let once = rule(start, Repeat::None, None);
        assert_eq!(once.latest_at_or_before(start - 1), None);
        assert_eq!(once.latest_at_or_before(start), Some(start));
        assert_eq!(once.latest_at_or_before(start + DAY_MS * 400), Some(start));
        assert_eq!(once.upcoming(start - 1, 10), vec![start]);
        assert_eq!(once.upcoming(start + 1, 10), Vec::<i64>::new());
    }

    #[test]
    fn daily_and_weekly_rules_step_by_fixed_lengths() {
        let start = beijing(2026, 11, 1, 9, 0);
        let daily = rule(start, Repeat::Daily, None);
        assert_eq!(
            daily.latest_at_or_before(start + 3 * DAY_MS + 3_600_000),
            Some(start + 3 * DAY_MS)
        );
        assert_eq!(
            daily.latest_at_or_before(start + 6 * DAY_MS),
            Some(start + 6 * DAY_MS)
        );
        assert_eq!(
            daily.upcoming(start, 3),
            vec![start, start + DAY_MS, start + 2 * DAY_MS]
        );
        assert_eq!(
            daily.upcoming(start + DAY_MS + 1, 2),
            vec![start + 2 * DAY_MS, start + 3 * DAY_MS]
        );

        let weekly = rule(start, Repeat::Weekly, None);
        assert_eq!(
            weekly.latest_at_or_before(start + 2 * WEEK_MS - 1),
            Some(start + WEEK_MS)
        );
        assert_eq!(
            weekly.latest_at_or_before(start + 2 * WEEK_MS),
            Some(start + 2 * WEEK_MS)
        );
        assert_eq!(
            weekly.upcoming(start, 3),
            vec![start, start + WEEK_MS, start + 2 * WEEK_MS]
        );
    }

    #[test]
    fn monthly_keeps_the_day_of_month_and_clamps_short_months_then_restores() {
        // 1 月 31 日开始的月重复。
        let start = beijing(2026, 1, 31, 9, 0);
        let monthly = rule(start, Repeat::Monthly, None);

        // 2 月没有 31 日：钳制到 2 月 28 日（2026 不是闰年）。
        assert_eq!(
            monthly.latest_at_or_before(beijing(2026, 3, 1, 0, 0)),
            Some(beijing(2026, 2, 28, 9, 0))
        );
        // 3 月有 31 日：恢复到原来的日号。
        assert_eq!(
            monthly.latest_at_or_before(beijing(2026, 4, 1, 0, 0)),
            Some(beijing(2026, 3, 31, 9, 0))
        );
        // 4 月 30 日。
        assert_eq!(
            monthly.latest_at_or_before(beijing(2026, 5, 15, 0, 0)),
            Some(beijing(2026, 4, 30, 9, 0))
        );
        // 闰年的 2 月有 29 日。
        assert_eq!(
            monthly.latest_at_or_before(beijing(2028, 3, 1, 0, 0)),
            Some(beijing(2028, 2, 29, 9, 0))
        );
        assert_eq!(
            monthly.latest_at_or_before(beijing(2028, 2, 15, 0, 0)),
            Some(beijing(2028, 1, 31, 9, 0))
        );

        assert_eq!(
            monthly.upcoming(start, 4),
            vec![
                start,
                beijing(2026, 2, 28, 9, 0),
                beijing(2026, 3, 31, 9, 0),
                beijing(2026, 4, 30, 9, 0),
            ]
        );
    }

    #[test]
    fn equivalent_offsets_land_on_the_same_beijing_date() {
        // 同一时刻的两种写法：北京时间 1 月 31 日 09:00，与 UTC 1 月 31 日 01:00。
        let mut written_locally = schedule(beijing(2026, 1, 31, 9, 0), Repeat::Monthly, None);
        written_locally.start_at = "2026-01-31T09:00:00+08:00".into();
        let mut written_in_utc = written_locally.clone();
        written_in_utc.start_at = "2026-01-31T01:00:00Z".into();
        let locally = Recurrence::from_schedule(&written_locally).unwrap();
        let utc = Recurrence::from_schedule(&written_in_utc).unwrap();
        assert_eq!(locally, utc);
        assert_eq!(
            locally.latest_at_or_before(beijing(2026, 3, 1, 0, 0)),
            utc.latest_at_or_before(beijing(2026, 3, 1, 0, 0))
        );
        assert_eq!(
            locally.upcoming(beijing(2026, 2, 1, 0, 0), 3),
            utc.upcoming(beijing(2026, 2, 1, 0, 0), 3)
        );
    }

    #[test]
    fn the_repeat_end_is_inclusive() {
        let start = beijing(2026, 1, 1, 8, 0);
        let third = start + 2 * DAY_MS;
        let limited = rule(start, Repeat::Daily, Some(third));
        // 截止时刻恰好是第三次出现：仍然有效，之后的片刻也还是它。
        assert_eq!(limited.latest_at_or_before(third), Some(third));
        assert_eq!(limited.latest_at_or_before(third + 1), Some(third));
        // 再往后就轮到第四次出现，它已经超出截止。
        assert_eq!(limited.latest_at_or_before(third + DAY_MS), None);
        // 只列出截止之前的出现。
        assert_eq!(
            limited.upcoming(start, 10),
            vec![start, start + DAY_MS, third]
        );
        // 截止早于第一次出现：一次都没有。
        let never = rule(start, Repeat::Daily, Some(start - 1));
        assert_eq!(never.latest_at_or_before(start + DAY_MS), None);
        assert!(never.upcoming(start, 10).is_empty());
    }

    #[test]
    fn a_monthly_rule_stops_at_its_repeat_end() {
        let start = beijing(2026, 1, 31, 9, 0);
        let until = beijing(2026, 4, 30, 9, 0);
        let monthly = rule(start, Repeat::Monthly, Some(until));
        assert_eq!(
            monthly.upcoming(start, 10),
            vec![
                start,
                beijing(2026, 2, 28, 9, 0),
                beijing(2026, 3, 31, 9, 0),
                beijing(2026, 4, 30, 9, 0),
            ]
        );
        // 截止在 4 月 29 日：4 月的这次不算。
        let earlier = rule(start, Repeat::Monthly, Some(beijing(2026, 4, 29, 9, 0)));
        assert_eq!(earlier.upcoming(start, 10).len(), 3);
    }

    #[test]
    fn schedules_with_unusable_times_never_occur() {
        let start = beijing(2026, 1, 1, 8, 0);
        for broken in [
            "",
            "not a date",
            "2026-01-01T08:00:00",
            "2026-13-01T08:00:00Z",
        ] {
            let mut data = schedule(start, Repeat::Daily, None);
            data.start_at = broken.into();
            assert_eq!(
                Recurrence::from_schedule(&data).unwrap_err(),
                RecurrenceError::InvalidStart,
                "{broken:?}"
            );
        }
        let mut data = schedule(start, Repeat::Daily, None);
        data.repeat_until = Some("whenever".into());
        assert_eq!(
            Recurrence::from_schedule(&data).unwrap_err(),
            RecurrenceError::InvalidRepeatUntil
        );
    }

    #[test]
    fn a_range_counts_occurrences_without_enumerating_them() {
        let start = beijing(2026, 1, 1, 8, 0);
        let daily = rule(start, Repeat::Daily, None);
        // 十天整：首尾都算。
        let range = occurrence_range(&daily, start, start + 9 * DAY_MS).unwrap();
        assert_eq!(
            range,
            OccurrenceRange {
                first: start,
                last: start + 9 * DAY_MS,
                count: 10
            }
        );
        // 从第二天中途开始：第一天的出现不算。
        let range = occurrence_range(&daily, start + DAY_MS + 1, start + 9 * DAY_MS).unwrap();
        assert_eq!(range.first, start + 2 * DAY_MS);
        assert_eq!(range.count, 8);
        // 区间整体在日程开始之前。
        assert_eq!(
            occurrence_range(&daily, start - 5 * DAY_MS, start - DAY_MS),
            None
        );
        // 区间整体在重复截止之后。
        let limited = rule(start, Repeat::Daily, Some(start + 2 * DAY_MS));
        assert_eq!(
            occurrence_range(&limited, start + 3 * DAY_MS, start + 9 * DAY_MS),
            None
        );
        // 截止把区间截短。
        let range = occurrence_range(&limited, start, start + 9 * DAY_MS).unwrap();
        assert_eq!(range.count, 3);
    }

    #[test]
    fn ranges_follow_the_beijing_calendar_for_monthly_rules() {
        let start = beijing(2026, 1, 31, 23, 30);
        let monthly = rule(start, Repeat::Monthly, None);
        // 到 2026 年 5 月 1 日为止：1、2、3、4 月各一次。
        let range = occurrence_range(&monthly, start, beijing(2026, 5, 1, 0, 0)).unwrap();
        assert_eq!(range.count, 4);
        assert_eq!(range.first, start);
        assert_eq!(range.last, beijing(2026, 4, 30, 23, 30));
    }

    #[test]
    fn crossing_the_beijing_midnight_keeps_the_wall_clock_time() {
        // UTC 17:00 是北京时间的次日 01:00。
        let start = beijing(2026, 3, 1, 1, 0);
        let monthly = rule(start, Repeat::Monthly, None);
        assert_eq!(
            monthly.upcoming(start, 3),
            vec![start, beijing(2026, 4, 1, 1, 0), beijing(2026, 5, 1, 1, 0),]
        );
    }

    #[test]
    fn far_future_asks_are_bounded() {
        let start = beijing(2026, 1, 1, 8, 0);
        let monthly = rule(start, Repeat::Monthly, None);
        let far = start + (MAX_STEPS + 5) * 31 * DAY_MS;
        // 序号被封顶，不会溢出，也不会给出超过步数上限的出现。
        assert!(monthly.latest_at_or_before(far).is_some());
        assert!(monthly.upcoming(far, 5).len() <= 5);
        assert!(
            rule(start, Repeat::Daily, None)
                .upcoming(start, 0)
                .is_empty()
        );
    }

    #[test]
    fn civil_dates_round_trip_across_leap_years_and_epoch_boundaries() {
        for (year, month, day) in [
            (1970, 0, 1),
            (1969, 11, 31),
            (2000, 1, 29),
            (1900, 1, 28),
            (2024, 11, 31),
            (1600, 0, 1),
            (9999, 11, 31),
        ] {
            let date = Civil { year, month, day };
            assert_eq!(Civil::from_days(date.to_days()), date, "{date:?}");
            let ms = date.to_ms();
            assert_eq!(ms.rem_euclid(DAY_MS), 0);
            assert_eq!(Civil::from_ms(ms), date);
        }
        assert!(is_leap(2000));
        assert!(!is_leap(1900));
        assert!(is_leap(2024));
        // 月份从 0 开始：0 是一月，1 是二月。
        assert_eq!(days_in_month(2026, 0), 31);
        assert_eq!(days_in_month(2024, 1), 29);
        assert_eq!(days_in_month(2026, 1), 28);
        assert_eq!(days_in_month(2026, 3), 30);
        assert_eq!(days_in_month(1900, 1), 28);
        assert_eq!(days_in_month(2000, 1), 29);
    }

    #[test]
    fn schedules_deserialise_from_the_wire_into_usable_rules() {
        let data: ScheduleData = serde_json::from_value(json!({
            "title": "周会",
            "startAt": "2026-11-02T10:00:00.000+08:00",
            "repeat": "weekly",
            "status": "upcoming",
            "futureField": 1
        }))
        .unwrap();
        let rule = Recurrence::from_schedule(&data).unwrap();
        assert_eq!(rule.repeat, Repeat::Weekly);
        assert_eq!(rule.repeat_until, None);
        assert_eq!(
            rule.upcoming(rule.start_at, 2),
            vec![rule.start_at, rule.start_at + WEEK_MS]
        );
    }
}
