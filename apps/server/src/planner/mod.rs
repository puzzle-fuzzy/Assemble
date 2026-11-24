//! 日程规划：日程、提醒，以及它们到点之后生成的通知。
//!
//! - **改动带着版本号。** 修改、改状态、删除都要带上看到时的那个版本；对不上就拒绝，
//!   并给一个稳定的错误码。两台设备各改各的，不会悄悄互相覆盖。
//! - **新建可以带幂等键。** 请求超时之后拿同一个键重试，拿回的是原来那一条，不会建出两条。
//!   这是助手写入时最要紧的一条：它重试一次，不该让日历上多出一条。
//! - **任何一处改动，整份名单推给这个账户的所有设备。** 不做增量合并：
//!   日程、提醒、未读通知加起来很小，整份推过去，各端直接替换。
//! - **通知落在数据库里，不只是推一把。** 事件流可能断；设备重新连上时拉一次名单，
//!   就能把断线期间到的通知补上。同一场只通知一次靠的是通知的编号：
//!   日程的通知编号由"日程 + 场次时刻"决定，数据库不会让同一个编号出现两行。
//!
//! 到点检查见 [`tick`]：每一场只通知一次，太晚的不补，提醒不管多晚都补。

pub mod notifications;
pub mod reminders;
pub mod schedules;
pub mod tick;

use std::sync::Arc;

use assemble_protocol::error::codes;
use assemble_protocol::events::Event;
use assemble_protocol::primitives::Timestamp;
use assemble_protocol::schedule::Planner;
use axum::Json;
use axum::extract::State;
use axum::http::header::HeaderMap;
use sqlx::SqliteExecutor;

use crate::auth::Identity;
use crate::error::ApiError;
use crate::events::{Hub, Message};
use crate::http::Shared;

/// 名单里保留最近多少毫秒之内改动过的记录：七天。
/// 刚标完成、刚取消的那一条不会当场从名单里消失，界面上还能看一眼、还能撤回。
pub const RECENT_MILLIS: i64 = 7 * 24 * 60 * 60 * 1000;

/// 每一类名单最多返回多少条。
pub const LISTING_LIMIT: i64 = 500;

/// 未读的通知最多返回多少条。
pub const NOTIFICATION_LIMIT: i64 = 100;

/// `GET /v1/planner`：这个账户的日程、提醒与还没读的通知。
pub async fn planner(
    State(state): State<Shared>,
    identity: Identity,
) -> Result<Json<Planner>, ApiError> {
    let planner = planner_of(&state.pool, &identity.account_id, Timestamp::now()).await?;
    Ok(Json(planner))
}

/// 组一份名单。推送事件与接口回答用的是同一个函数，两边看到的永远一致。
pub async fn planner_of(
    pool: &sqlx::SqlitePool,
    account_id: &str,
    now: Timestamp,
) -> Result<Planner, sqlx::Error> {
    let since = now.as_millis() - RECENT_MILLIS;
    let schedules: Vec<schedules::ScheduleRow> = sqlx::query_as(
        "SELECT * FROM schedules \
         WHERE account_id = ? AND (status = 'upcoming' OR updated_at >= ?) \
         ORDER BY start_at, rowid LIMIT ?",
    )
    .bind(account_id)
    .bind(since)
    .bind(LISTING_LIMIT)
    .fetch_all(pool)
    .await?;
    let reminders: Vec<reminders::ReminderRow> = sqlx::query_as(
        "SELECT * FROM reminders \
         WHERE account_id = ? \
         AND (status IN ('scheduled', 'fired') OR updated_at >= ?) \
         ORDER BY due_at, rowid LIMIT ?",
    )
    .bind(account_id)
    .bind(since)
    .bind(LISTING_LIMIT)
    .fetch_all(pool)
    .await?;
    let notifications: Vec<notifications::NotificationRow> = sqlx::query_as(
        "SELECT * FROM notifications \
         WHERE account_id = ? AND read_at IS NULL AND invalidated_at IS NULL AND raised_at >= ? \
         ORDER BY raised_at DESC, rowid DESC LIMIT ?",
    )
    .bind(account_id)
    .bind(since)
    .bind(NOTIFICATION_LIMIT)
    .fetch_all(pool)
    .await?;
    Ok(Planner {
        schedules: schedules.iter().map(schedules::ScheduleRow::view).collect(),
        reminders: reminders.iter().map(reminders::ReminderRow::view).collect(),
        notifications: notifications
            .iter()
            .map(notifications::NotificationRow::view)
            .collect(),
    })
}

/// 一处改动之后，把整份名单推给这个账户的所有设备。
pub async fn publish(hub: &Hub, pool: &sqlx::SqlitePool, account_id: &str) {
    let planner = match planner_of(pool, account_id, Timestamp::now()).await {
        Ok(planner) => planner,
        Err(error) => {
            // 改动已经落盘，只是没能推出去：设备下次对账时会拉到。记下来，不让调用方失败。
            tracing::error!(%error, "the planner could not be listed for an event");
            return;
        }
    };
    hub.publish(
        account_id,
        Message::Event {
            audience: None,
            event: Arc::new(Event::PlannerChanged { planner }),
        },
    );
}

/// 枚举在线上的拼法。
pub fn spelling<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// 从线上的拼法读回枚举。库里存了不认识的值时返回 `None`，由调用方决定怎么收场。
pub fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}

/// 库里的毫秒数换回时间点。
pub fn time(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap_or_else(Timestamp::now)
}

/// `Idempotency-Key` 请求头：可以不带；带了就得是 1 到 128 个字母、数字、点、下划线或横线。
pub fn idempotency_key(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    let Some(value) = headers.get("idempotency-key") else {
        return Ok(None);
    };
    let text = value
        .to_str()
        .map_err(|_| ApiError::invalid("the Idempotency-Key header is not valid text"))?;
    if is_mutation_key(text) {
        Ok(Some(text.to_owned()))
    } else {
        Err(ApiError::invalid(
            "the Idempotency-Key header must be 1 to 128 characters of letters, digits, '.', '_' or '-'",
        ))
    }
}

fn is_mutation_key(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 128
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// 这个账户用这个键建过的那一条。
pub async fn mutated<'e, E>(
    executor: E,
    account_id: &str,
    key: &str,
    resource: &str,
) -> Result<Option<String>, sqlx::Error>
where
    E: SqliteExecutor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query_scalar::<_, String>(
        "SELECT resource_id FROM planner_mutations WHERE account_id = ? AND key = ? AND resource = ?",
    )
    .bind(account_id)
    .bind(key)
    .bind(resource)
    .fetch_optional(executor)
    .await
}

/// 记下一个用过的键。
pub async fn remember(
    executor: impl SqliteExecutor<'_, Database = sqlx::Sqlite>,
    account_id: &str,
    key: &str,
    resource: &str,
    resource_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO planner_mutations (account_id, key, resource, resource_id) VALUES (?, ?, ?, ?)",
    )
    .bind(account_id)
    .bind(key)
    .bind(resource)
    .bind(resource_id)
    .execute(executor)
    .await?;
    Ok(())
}

pub fn version_conflict() -> ApiError {
    ApiError::conflict(
        codes::VERSION_CONFLICT,
        "the record was changed elsewhere; fetch it again",
    )
}

pub fn invalid_state(message: impl Into<String>) -> ApiError {
    ApiError::conflict(codes::INVALID_STATE, message)
}

pub fn not_acceptable(what: &str) -> ApiError {
    ApiError::invalid(format!("the {what} is not acceptable"))
}
