//! 通知：日程与提醒到点之后生成的那一条，以及它的作废与已读。
//!
//! 通知一旦生成就留在库里：读过与作废的只打上时间，不删。
//! 删了的话，同一场又会被当成没通知过。

use assemble_protocol::primitives::{Timestamp, is_valid_id};
use assemble_protocol::schedule::{Notification, NotificationKind, ReadInput, ScheduleContent};
use assemble_schedule::occurrences::Recurrence;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use sqlx::SqliteExecutor;

use super::{parse, publish, time};
use crate::audit::{self, actions};
use crate::auth::Identity;
use crate::error::ApiError;
use crate::http::{self, Shared};

/// 一次最多把多少条通知标成已读。
pub const READ_LIMIT: usize = 200;

/// 一条通知在库里的样子。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct NotificationRow {
    pub id: String,
    pub account_id: String,
    pub kind: String,
    pub title: String,
    pub body: Option<String>,
    pub schedule_id: Option<String>,
    pub reminder_id: Option<String>,
    pub occurrence_at: Option<i64>,
    pub raised_at: i64,
}

impl NotificationRow {
    pub fn view(&self) -> Notification {
        Notification {
            id: self.id.clone(),
            kind: parse(&self.kind).unwrap_or(NotificationKind::Reminder),
            title: self.title.clone(),
            body: self.body.clone(),
            schedule_id: self.schedule_id.clone(),
            reminder_id: self.reminder_id.clone(),
            occurrence_at: self.occurrence_at.map(time),
            raised_at: time(self.raised_at),
        }
    }
}

/// 日程某一场的通知正文：备注在前，地点在后。
pub fn schedule_body(content: &ScheduleContent) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(notes) = &content.notes {
        lines.push(notes.clone());
    }
    if let Some(location) = &content.location {
        lines.push(format!("{} {}", location.label, location.address));
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// 把一条日程还没读、也没作废的通知全部作废。取消、完成、删除时用。
pub async fn invalidate_for_schedule(
    executor: impl SqliteExecutor<'_, Database = sqlx::Sqlite>,
    schedule_id: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE notifications SET invalidated_at = ? \
         WHERE schedule_id = ? AND read_at IS NULL AND invalidated_at IS NULL",
    )
    .bind(now)
    .bind(schedule_id)
    .execute(executor)
    .await?;
    Ok(())
}

/// 把一条提醒还没读、也没作废的通知作废。
pub async fn invalidate_for_reminder(
    executor: impl SqliteExecutor<'_, Database = sqlx::Sqlite>,
    reminder_id: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE notifications SET invalidated_at = ? \
         WHERE reminder_id = ? AND read_at IS NULL AND invalidated_at IS NULL",
    )
    .bind(now)
    .bind(reminder_id)
    .execute(executor)
    .await?;
    Ok(())
}

/// 日程改了时间或重复规则之后：新规则下不再成立的那些场次，通知作废。
///
/// 还成立的场次（比如今早已经提醒过的那一场）不动——改的是明天的安排，今早的提醒仍是事实。
pub async fn drop_stale(
    connection: &mut sqlx::SqliteConnection,
    schedule_id: &str,
    content: &ScheduleContent,
    now: i64,
) -> Result<(), sqlx::Error> {
    let rule = Recurrence::new(
        content.start_at.as_millis(),
        content.repeat,
        content.repeat_until.map(Timestamp::as_millis),
    );
    let live: Vec<(String, i64)> = sqlx::query_as(
        "SELECT id, occurrence_at FROM notifications \
         WHERE schedule_id = ? AND read_at IS NULL AND invalidated_at IS NULL \
         AND occurrence_at IS NOT NULL",
    )
    .bind(schedule_id)
    .fetch_all(&mut *connection)
    .await?;
    for (id, occurrence) in live {
        if !rule.occurs_at(occurrence) {
            sqlx::query("UPDATE notifications SET invalidated_at = ? WHERE id = ?")
                .bind(now)
                .bind(id)
                .execute(&mut *connection)
                .await?;
        }
    }
    Ok(())
}

/// 日程只改了标题、地点或备注：还没读的通知原地更新，编号不变。
pub async fn refresh(
    executor: impl SqliteExecutor<'_, Database = sqlx::Sqlite>,
    schedule_id: &str,
    content: &ScheduleContent,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE notifications SET title = ?, body = ? \
         WHERE schedule_id = ? AND read_at IS NULL AND invalidated_at IS NULL",
    )
    .bind(&content.title)
    .bind(schedule_body(content))
    .bind(schedule_id)
    .execute(executor)
    .await?;
    Ok(())
}

/// `POST /v1/notifications/read`：把几条通知标成已读。
pub async fn read(
    State(state): State<Shared>,
    identity: Identity,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let input: ReadInput = http::read_json(&body)?;
    if input.ids.len() > READ_LIMIT {
        return Err(ApiError::invalid(
            "at most 200 notifications can be marked read at once",
        ));
    }
    if input.ids.iter().any(|id| !is_valid_id(id)) {
        return Err(ApiError::invalid("a notification id is not acceptable"));
    }
    if input.ids.is_empty() {
        return Ok(StatusCode::NO_CONTENT);
    }
    // 一条一条地改：语句是写死的，标识只经绑定进去。
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    for id in &input.ids {
        sqlx::query(
            "UPDATE notifications SET read_at = ? \
             WHERE account_id = ? AND read_at IS NULL AND id = ?",
        )
        .bind(Timestamp::now().as_millis())
        .bind(&identity.account_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        actions::NOTIFICATION_READ,
        None,
    )
    .await;
    publish(&state.hub, &state.pool, &identity.account_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/notifications/read-all`：这个账户的通知全部标成已读。
pub async fn read_all(
    State(state): State<Shared>,
    identity: Identity,
) -> Result<StatusCode, ApiError> {
    sqlx::query(
        "UPDATE notifications SET read_at = ? \
         WHERE account_id = ? AND read_at IS NULL AND invalidated_at IS NULL",
    )
    .bind(Timestamp::now().as_millis())
    .bind(&identity.account_id)
    .execute(&state.pool)
    .await?;
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        actions::NOTIFICATION_READ,
        None,
    )
    .await;
    publish(&state.hub, &state.pool, &identity.account_id).await;
    Ok(StatusCode::NO_CONTENT)
}
