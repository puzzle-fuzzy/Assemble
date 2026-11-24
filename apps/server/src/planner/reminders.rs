//! 提醒：一件到点要做的事。一次性的，不重复。
//!
//! 状态有四种。`fired` 只由服务端在到点时写入；到点之前可以取消，也可以提前办完
//! （同样算 `dismissed`）。到点之后不能再改时间——旧的那条提醒已经发出去了。

use assemble_protocol::primitives::Timestamp;
use assemble_protocol::schedule::{
    Reminder, ReminderContent, ReminderStatus, ReminderUpdate, Versioned,
};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use sqlx::SqliteExecutor;

use super::notifications;
use super::{
    idempotency_key, invalid_state, mutated, not_acceptable, parse, publish, remember, time,
    version_conflict,
};
use crate::audit::{self, actions};
use crate::auth::Identity;
use crate::error::ApiError;
use crate::http::{self, Shared};

const RESOURCE: &str = "reminder";

/// 一条提醒在库里的样子。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReminderRow {
    pub id: String,
    pub account_id: String,
    pub title: String,
    pub due_at: i64,
    pub notes: Option<String>,
    pub status: String,
    pub version: i64,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ReminderRow {
    pub fn content(&self) -> ReminderContent {
        ReminderContent {
            title: self.title.clone(),
            due_at: time(self.due_at),
            notes: self.notes.clone(),
        }
    }

    pub fn view(&self) -> Reminder {
        Reminder {
            id: self.id.clone(),
            version: self.version,
            // 库里存了不认识的状态：当作已取消，至少不会再去提醒。
            status: parse(&self.status).unwrap_or(ReminderStatus::Cancelled),
            created_by: self.created_by.clone(),
            created_at: time(self.created_at),
            updated_at: time(self.updated_at),
            content: self.content(),
        }
    }
}

/// 按标识取出这个账户的一条提醒；不是这个账户的，与不存在同样回答。
pub async fn load(
    executor: impl SqliteExecutor<'_, Database = sqlx::Sqlite>,
    account_id: &str,
    id: &str,
) -> Result<ReminderRow, ApiError> {
    sqlx::query_as::<_, ReminderRow>("SELECT * FROM reminders WHERE account_id = ? AND id = ?")
        .bind(account_id)
        .bind(id)
        .fetch_optional(executor)
        .await?
        .ok_or_else(ApiError::not_found)
}

/// `POST /v1/reminders`：新建一条提醒。带了 `Idempotency-Key` 而且这个键用过，拿回原来那一条。
pub async fn create(
    State(state): State<Shared>,
    identity: Identity,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let input: ReminderContent = http::read_json(&body)?;
    let Some(content) = input.cleaned() else {
        return Err(not_acceptable(RESOURCE));
    };
    let key = idempotency_key(&headers)?;
    if let Some(key) = &key
        && let Some(existing) = mutated(&state.pool, &identity.account_id, key, RESOURCE).await?
    {
        let row = load(&state.pool, &identity.account_id, &existing).await?;
        return Ok((StatusCode::OK, Json(row.view())).into_response());
    }

    let id = crate::secrets::random_id().map_err(|error| ApiError::internal(error.to_string()))?;
    let now = Timestamp::now().as_millis();
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    // 两个请求带着同一个键同时到：以事务里看到的为准，只认先到的那个。
    if let Some(key) = &key
        && let Some(existing) = mutated(&mut *tx, &identity.account_id, key, RESOURCE).await?
    {
        let row = load(&state.pool, &identity.account_id, &existing).await?;
        return Ok((StatusCode::OK, Json(row.view())).into_response());
    }
    sqlx::query(
        "INSERT INTO reminders \
         (id, account_id, title, due_at, notes, status, version, created_by, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, 'scheduled', 1, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&identity.account_id)
    .bind(&content.title)
    .bind(content.due_at.as_millis())
    .bind(&content.notes)
    .bind(&identity.device_name)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    if let Some(key) = &key {
        remember(&mut *tx, &identity.account_id, key, RESOURCE, &id).await?;
    }
    tx.commit().await?;
    let row = load(&state.pool, &identity.account_id, &id).await?;
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        actions::REMINDER_CREATED,
        Some(&row.id),
    )
    .await;
    publish(&state.hub, &state.pool, &identity.account_id).await;
    Ok((StatusCode::CREATED, Json(row.view())).into_response())
}

/// `PUT /v1/reminders/{id}`：整份替换内容。已经到点的提醒不能再改。
pub async fn update(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Reminder>, ApiError> {
    let input: ReminderUpdate = http::read_json(&body)?;
    let Some(content) = input.content.cleaned() else {
        return Err(not_acceptable(RESOURCE));
    };
    let now = Timestamp::now().as_millis();
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let row = load(&mut *tx, &identity.account_id, &id).await?;
    if row.version != input.version {
        return Err(version_conflict());
    }
    if row.status != "scheduled" {
        return Err(invalid_state(
            "a reminder that has fired, been dismissed or been cancelled cannot be edited",
        ));
    }
    sqlx::query(
        "UPDATE reminders SET title = ?, due_at = ?, notes = ?, version = ?, updated_at = ? \
         WHERE id = ?",
    )
    .bind(&content.title)
    .bind(content.due_at.as_millis())
    .bind(&content.notes)
    .bind(row.version + 1)
    .bind(now)
    .bind(&row.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let updated = load(&state.pool, &identity.account_id, &row.id).await?;
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        actions::REMINDER_UPDATED,
        Some(&updated.id),
    )
    .await;
    publish(&state.hub, &state.pool, &identity.account_id).await;
    Ok(Json(updated.view()))
}

/// 改状态。只有 `allowed` 里的状态能改成 `target`；版本号对不上就被拒绝。
async fn change_status(
    state: &Shared,
    identity: &Identity,
    id: &str,
    input: Versioned,
    target: &str,
    allowed: &[&str],
    action: &'static str,
) -> Result<Reminder, ApiError> {
    let now = Timestamp::now().as_millis();
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let row = load(&mut *tx, &identity.account_id, id).await?;
    if row.version != input.version {
        return Err(version_conflict());
    }
    if row.status == target {
        // 已经是这个状态：什么都不做，也不涨版本号。
    } else if allowed.contains(&row.status.as_str()) {
        // 提醒作废之后，还没读的那条通知不再补发给任何设备。
        if target == "cancelled" || target == "dismissed" {
            notifications::invalidate_for_reminder(&mut *tx, &row.id, now).await?;
        }
        sqlx::query(
            "UPDATE reminders SET status = ?, version = version + 1, updated_at = ? WHERE id = ?",
        )
        .bind(target)
        .bind(now)
        .bind(&row.id)
        .execute(&mut *tx)
        .await?;
    } else {
        return Err(invalid_state(
            "the reminder is not in a state that allows this",
        ));
    }
    tx.commit().await?;
    let changed = load(&state.pool, &identity.account_id, &row.id).await?;
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        action,
        Some(&changed.id),
    )
    .await;
    publish(&state.hub, &state.pool, &identity.account_id).await;
    Ok(changed.view())
}

fn versioned(body: &[u8]) -> Result<Versioned, ApiError> {
    http::read_json(body)
}

/// `POST /v1/reminders/{id}/dismiss`：办完了，或者到点之后看过了。
pub async fn dismiss(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Reminder>, ApiError> {
    change_status(
        &state,
        &identity,
        &id,
        versioned(&body)?,
        "dismissed",
        &["scheduled", "fired"],
        actions::REMINDER_DISMISSED,
    )
    .await
    .map(Json)
}

/// `POST /v1/reminders/{id}/cancel`：不提醒了。
pub async fn cancel(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Reminder>, ApiError> {
    change_status(
        &state,
        &identity,
        &id,
        versioned(&body)?,
        "cancelled",
        &["scheduled", "fired"],
        actions::REMINDER_CANCELLED,
    )
    .await
    .map(Json)
}

/// `DELETE /v1/reminders/{id}`：连它的通知一起删掉。
pub async fn remove(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let input: Versioned = http::read_json(&body)?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let row = load(&mut *tx, &identity.account_id, &id).await?;
    if row.version != input.version {
        return Err(version_conflict());
    }
    sqlx::query("DELETE FROM notifications WHERE reminder_id = ?")
        .bind(&row.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM reminders WHERE id = ?")
        .bind(&row.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        actions::REMINDER_DELETED,
        Some(&row.id),
    )
    .await;
    publish(&state.hub, &state.pool, &identity.account_id).await;
    Ok(StatusCode::NO_CONTENT)
}
