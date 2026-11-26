//! 给手机应用的样本：日程重复规则的推算结果。手机上的本地通知要自己算"下一场是什么时候"，
//! 那一份算法是移植过去的；这里用各种各样的起点、规则和询问时刻把 Rust 这一份的结果
//! 写成 JSON，Swift 那边的测试必须算出完全相同的结果。月重复的钳制、跨日界、闰年、
//! 重复截止的边界都在里面。刷新样本用 `ASSEMBLE_UPDATE_FIXTURES=1`。

use std::path::PathBuf;

use assemble_schedule::model::Repeat;
use assemble_schedule::occurrences::{Recurrence, SHANGHAI_OFFSET_MS};
use chrono::{DateTime, FixedOffset, TimeZone};
use serde_json::{Value, json};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/ios/Fixtures/recurrence.json")
}

/// 北京时间某年某月某日某时某分对应的毫秒时间戳。
fn beijing(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
    FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(year, month, day, hour, minute, 0)
        .unwrap()
        .timestamp_millis()
}

fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
    chrono::Utc
        .with_ymd_and_hms(year, month, day, hour, minute, 0)
        .unwrap()
        .timestamp_millis()
}

fn iso(ms: i64) -> String {
    DateTime::from_timestamp_millis(ms).unwrap().to_rfc3339()
}

fn spelled(repeat: Repeat) -> &'static str {
    match repeat {
        Repeat::None => "none",
        Repeat::Daily => "daily",
        Repeat::Weekly => "weekly",
        Repeat::Monthly => "monthly",
    }
}

/// 各种各样的起点：月末、闰年、跨北京时间的日界、纪元之前。
fn starts() -> Vec<(&'static str, i64)> {
    vec![
        ("month-end", beijing(2026, 1, 31, 9, 0)),
        ("leap-month-end", beijing(2028, 1, 31, 9, 0)),
        ("across-beijing-midnight", beijing(2026, 3, 1, 1, 0)),
        ("leap-day-noon", beijing(2024, 2, 29, 12, 0)),
        ("thirty-first-at-midnight", beijing(2026, 10, 31, 0, 0)),
        ("year-end", beijing(2026, 12, 31, 23, 59)),
        ("written-in-utc", utc(2026, 11, 2, 2, 0)),
        ("before-the-epoch", utc(1969, 12, 31, 23, 30)),
    ]
}

fn step(repeat: Repeat) -> i64 {
    match repeat {
        Repeat::None => 0,
        Repeat::Daily => 86_400_000,
        Repeat::Weekly => 7 * 86_400_000,
        // 月重复的步长不固定，只拿它当"一段距离"用，不指望落在某一次出现上。
        Repeat::Monthly => 30 * 86_400_000,
    }
}

fn sample() -> Value {
    let mut rules = Vec::new();
    for (name, start) in starts() {
        for repeat in [Repeat::None, Repeat::Daily, Repeat::Weekly, Repeat::Monthly] {
            // "截止早于第一次出现"只需要一种规则来代表：结果都是空的。
            let untils: Vec<(&str, Option<i64>)> = if repeat == Repeat::Daily {
                vec![
                    ("open", None),
                    ("after-three", Some(start + 3 * step(repeat))),
                    ("never", Some(start - 1)),
                ]
            } else {
                vec![
                    ("open", None),
                    ("after-three", Some(start + 3 * step(repeat))),
                ]
            };
            for (until_name, repeat_until) in untils {
                let rule = Recurrence::new(start, repeat, repeat_until);
                let length = step(repeat);
                // 询问的时刻落在出现的前一刻、当刻、后一刻，以及两次出现之间。
                let mut points = vec![start - 1, start, start + 1, start + length / 2];
                for times in [1, 3, 40] {
                    points.push(start + times * length + 1);
                }
                points.push(start + 3 * length - 1);
                if repeat == Repeat::Monthly {
                    points.push(start + (12_000 + 5) * 31 * 86_400_000);
                }
                let asks: Vec<Value> = points
                    .iter()
                    .map(|point| {
                        json!({
                            "from": iso(*point),
                            "limit": 4,
                            "upcoming": rule.upcoming(*point, 4).iter().map(|ms| iso(*ms)).collect::<Vec<_>>(),
                            "latest": rule.latest_at_or_before(*point).map(iso),
                        })
                    })
                    .collect();
                rules.push(json!({
                    "name": format!("{name}-{until_name}"),
                    "startAt": iso(start),
                    "repeat": spelled(repeat),
                    "repeatUntil": repeat_until.map(iso),
                    "asks": asks,
                }));
            }
        }
    }
    json!({
        "shanghaiOffsetMs": SHANGHAI_OFFSET_MS,
        "rules": rules,
    })
}

#[test]
fn the_phone_fixtures_match_the_rust_recurrence_rules() {
    let path = fixture_path();
    let rendered = format!(
        "{}\n",
        serde_json::to_string_pretty(&sample()).expect("the sample serialises")
    );
    if std::env::var_os("ASSEMBLE_UPDATE_FIXTURES").is_some_and(|value| value == "1") {
        std::fs::write(&path, &rendered)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        return;
    }
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    assert_eq!(
        committed, rendered,
        "the recurrence rules changed; refresh the phone fixtures with ASSEMBLE_UPDATE_FIXTURES=1"
    );
}
