//! 给手机应用的样本。这里把线上的类型按各种取值序列化成一份 JSON，Swift 那边的测试
//! 必须能把它原样解码。样本是 Rust 侧生成的，所以它就是线上的格式本身：协议改了而
//! 样本没跟上，这条测试先红；刷新样本用 `ASSEMBLE_UPDATE_FIXTURES=1`。

use std::path::PathBuf;

use assemble_protocol::account::Account;
use assemble_protocol::device::Device;
use assemble_protocol::login::MeResponse;
use assemble_protocol::pairing::{
    PairingClaim, PairingClaimed, PairingFinish, PairingOutcome, PairingState,
};
use assemble_protocol::primitives::{Platform, Timestamp};
use assemble_protocol::schedule::{
    Location, Notification, NotificationKind, Planner, Reminder, ReminderContent, ReminderStatus,
    Repeat, Schedule, ScheduleContent, ScheduleStatus,
};
use serde_json::{Value, json};

const CREATED: i64 = 1_794_400_000_000;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/iphone/Fixtures/protocol.json")
}

fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).expect("a valid instant")
}

fn account() -> Account {
    Account {
        id: "u_8654646cf7d77493d56688e6".to_owned(),
        username: "烟熏三文鱼".to_owned(),
        display_name: "烟熏三文鱼".to_owned(),
        email: Some("someone@example.test".to_owned()),
        avatar_url: Some("https://avatars.githubusercontent.com/u/1?s=96".to_owned()),
        created_at: at(CREATED),
    }
}

fn device(platform: Platform, online: bool, last_seen: Option<i64>) -> Device {
    let name = match platform {
        Platform::Iphone => "我的 iPhone",
        Platform::Other => "将来的设备",
        _ => "工作电脑",
    };
    Device {
        id: format!("device-{platform:?}"),
        name: name.to_owned(),
        platform,
        online,
        last_seen_at: last_seen.map(at),
        created_at: at(CREATED),
    }
}

fn phone() -> Device {
    device(Platform::Iphone, false, None)
}

fn content(
    title: &str,
    start_at: i64,
    repeat: Repeat,
    remind_before: Option<u32>,
) -> ScheduleContent {
    ScheduleContent {
        title: title.to_owned(),
        location: None,
        start_at: at(start_at),
        end_at: None,
        repeat,
        repeat_until: None,
        remind_before,
        notes: None,
    }
}

fn schedule(id: &str, content: ScheduleContent, status: ScheduleStatus, version: i64) -> Schedule {
    Schedule {
        id: id.to_owned(),
        version,
        status,
        created_by: "ZCode 的助手".to_owned(),
        created_at: at(CREATED),
        updated_at: at(CREATED + 100_000),
        content,
    }
}

fn schedules() -> Vec<Schedule> {
    let train = ScheduleContent {
        location: Some(Location {
            label: "杭州东站".to_owned(),
            address: "浙江省杭州市上城区全福桥路2号".to_owned(),
        }),
        notes: Some("带身份证".to_owned()),
        ..content("火车", 1_795_741_200_000, Repeat::None, Some(90))
    };
    let emoji = ScheduleContent {
        title: "与 🐟 的会面：季度回顾".to_owned(),
        ..content("", 1_794_800_000_000, Repeat::None, None)
    };
    let daily = ScheduleContent {
        end_at: Some(at(1_794_301_800_000)),
        repeat_until: Some(at(1_796_840_000_000)),
        ..content("站会", 1_794_300_000_000, Repeat::Daily, None)
    };
    let weekly = content("周会", 1_794_400_000_000, Repeat::Weekly, Some(15));
    let monthly = content("还信用卡", 1_768_143_400_000, Repeat::Monthly, None);
    let finished = content("已经结束的", 1_794_000_000_000, Repeat::None, None);
    vec![
        schedule(
            "c4cf4220b94fddee4f2fe490124b3fa9",
            train,
            ScheduleStatus::Upcoming,
            1,
        ),
        schedule("s-emoji", emoji, ScheduleStatus::Upcoming, 7),
        schedule("s-daily", daily, ScheduleStatus::Upcoming, 2),
        schedule("s-weekly", weekly, ScheduleStatus::Upcoming, 1),
        schedule("s-monthly", monthly, ScheduleStatus::Upcoming, 3),
        schedule("s-done", finished.clone(), ScheduleStatus::Done, 2),
        schedule("s-cancelled", finished, ScheduleStatus::Cancelled, 2),
    ]
}

fn reminder(
    id: &str,
    title: &str,
    due_at: i64,
    notes: Option<&str>,
    status: ReminderStatus,
    version: i64,
) -> Reminder {
    Reminder {
        id: id.to_owned(),
        version,
        status,
        created_by: "测试的手机".to_owned(),
        created_at: at(CREATED - 2_000_000),
        updated_at: at(due_at),
        content: ReminderContent {
            title: title.to_owned(),
            due_at: at(due_at),
            notes: notes.map(ToOwned::to_owned),
        },
    }
}

fn reminders() -> Vec<Reminder> {
    vec![
        reminder(
            "r-soon",
            "取快递",
            1_794_406_000_000,
            Some("驿站 8 点关门\n带上取件码"),
            ReminderStatus::Scheduled,
            1,
        ),
        reminder(
            "r-overdue",
            "已经过了点还没人处理的",
            1_794_350_000_000,
            None,
            ReminderStatus::Scheduled,
            1,
        ),
        reminder(
            "r-fired",
            "到过点的",
            1_794_260_000_000,
            None,
            ReminderStatus::Fired,
            2,
        ),
        reminder(
            "r-dismissed",
            "看过了的",
            1_794_150_000_000,
            None,
            ReminderStatus::Dismissed,
            3,
        ),
        reminder(
            "r-cancelled",
            "不办了的",
            1_794_120_000_000,
            None,
            ReminderStatus::Cancelled,
            2,
        ),
    ]
}

#[allow(clippy::too_many_arguments, reason = "样本就是把每一个字段都摆出来")]
fn notification(
    id: &str,
    kind: NotificationKind,
    title: &str,
    body: Option<&str>,
    schedule_id: Option<&str>,
    reminder_id: Option<&str>,
    occurrence_at: Option<i64>,
    raised_at: i64,
) -> Notification {
    Notification {
        id: id.to_owned(),
        kind,
        title: title.to_owned(),
        body: body.map(ToOwned::to_owned),
        schedule_id: schedule_id.map(ToOwned::to_owned),
        reminder_id: reminder_id.map(ToOwned::to_owned),
        occurrence_at: occurrence_at.map(at),
        raised_at: at(raised_at),
    }
}

fn notifications() -> Vec<Notification> {
    vec![
        notification(
            "c4cf4220b94fddee4f2fe490124b3fa9-1795741200000",
            NotificationKind::Schedule,
            "火车",
            Some("带身份证\n杭州东站 浙江省杭州市上城区全福桥路2号"),
            Some("c4cf4220b94fddee4f2fe490124b3fa9"),
            None,
            Some(1_795_741_200_000),
            1_795_738_800_000,
        ),
        notification(
            "n-r-soon",
            NotificationKind::Reminder,
            "取快递",
            Some("驿站 8 点关门\n带上取件码"),
            None,
            Some("r-soon"),
            None,
            1_794_406_000_000,
        ),
        notification(
            "s-daily-1794391200000",
            NotificationKind::Schedule,
            "站会",
            None,
            Some("s-daily"),
            None,
            Some(1_794_391_200_000),
            1_794_391_200_000,
        ),
    ]
}

/// 出错时正文的样子：手机应用靠 `error.code` 分辨该怎么办。
fn errors() -> Value {
    json!([
        { "status": 400, "body": { "error": { "code": "INVALID_AUTH_CODE", "message": "the pairing ticket is invalid, already used or expired" } } },
        { "status": 429, "body": { "error": { "code": "TOO_MANY_ATTEMPTS", "message": "too many pairing attempts from this address" } } },
        { "status": 401, "body": { "error": { "code": "UNAUTHORIZED", "message": "the device token is missing or invalid" } } },
        { "status": 409, "body": { "error": { "code": "VERSION_CONFLICT", "message": "the record changed since it was read" } } },
        { "status": 409, "body": { "error": { "code": "INVALID_STATE", "message": "the record is not in a state where this is allowed" } } },
    ])
}

fn sample() -> Value {
    json!({
        "account": account(),
        "devices": [
            device(Platform::Windows, true, Some(1_794_390_000_000)),
            device(Platform::Macos, false, Some(1_794_395_000_000)),
            device(Platform::Linux, false, None),
            phone(),
            device(Platform::Other, false, None),
        ],
        "me": MeResponse {
            account: account(),
            device: phone(),
        },
        "pairingClaim": PairingClaim {
            ticket: "abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE".to_owned(),
            code_challenge: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_".to_owned(),
            device_name: "我的 iPhone".to_owned(),
        },
        "pairingFinish": PairingFinish {
            ticket: "abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE".to_owned(),
            code_verifier: "v".repeat(43),
        },
        "pairingClaimed": PairingClaimed {
            confirmation_code: "004757".to_owned(),
            expires_at: at(CREATED + 180_000),
        },
        "pairingOutcomes": {
            "pending": PairingOutcome::pending(),
            "denied": PairingOutcome::denied(),
            "complete": PairingOutcome {
                state: PairingState::Complete,
                account: Some(account()),
                device: Some(phone()),
                token: Some("t".repeat(43)),
            },
        },
        "planner": Planner {
            schedules: schedules(),
            reminders: reminders(),
            notifications: notifications(),
        },
        "errors": errors(),
    })
}

#[test]
fn the_phone_fixtures_match_the_wire_format() {
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
        "the protocol changed; refresh the phone fixtures with ASSEMBLE_UPDATE_FIXTURES=1"
    );
}
