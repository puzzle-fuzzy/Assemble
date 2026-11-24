//! 日程：一段有开始时间的安排，可以按天、周、月重复。
//!
//! 状态只有三种：进行中、已完成、已取消。"已完成"是整条日程的，不能只完成重复日程的某一场。
//! 改时间或重复规则时，新规则下不再成立的那些场次的提醒会被作废（见 `notifications`）。

use assemble_protocol::primitives::Timestamp;
use assemble_protocol::schedule::{
    Location, Repeat, Schedule, ScheduleContent, ScheduleStatus, ScheduleUpdate, Versioned,
};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use sqlx::SqliteExecutor;

use super::notifications;
use super::{
    idempotency_key, invalid_state, mutated, not_acceptable, parse, publish, remember, spelling,
    time, version_conflict,
};
use crate::audit::{self, actions};
use crate::auth::Identity;
use crate::error::ApiError;
use crate::http::{self, Shared};

const RESOURCE: &str = "schedule";

/// 一条日程在库里的样子。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ScheduleRow {
    pub id: String,
    pub account_id: String,
    pub title: String,
    pub location_label: Option<String>,
    pub location_address: Option<String>,
    pub start_at: i64,
    pub end_at: Option<i64>,
    pub repeat: String,
    pub repeat_until: Option<i64>,
    pub remind_before: Option<i64>,
    pub notes: Option<String>,
    pub status: String,
    pub version: i64,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ScheduleRow {
    pub fn content(&self) -> ScheduleContent {
        ScheduleContent {
            title: self.title.clone(),
            location: match (&self.location_label, &self.location_address) {
                (Some(label), Some(address)) => Some(Location {
                    label: label.clone(),
                    address: address.clone(),
                }),
                _ => None,
            },
            start_at: time(self.start_at),
            end_at: self.end_at.map(time),
            repeat: parse(&self.repeat).unwrap_or(Repeat::None),
            repeat_until: self.repeat_until.map(time),
            remind_before: self
                .remind_before
                .and_then(|minutes| u32::try_from(minutes).ok()),
            notes: self.notes.clone(),
        }
    }

    pub fn view(&self) -> Schedule {
        Schedule {
            id: self.id.clone(),
            version: self.version,
            // 库里存了不认识的状态：当作已取消，至少不会再去提醒。
            status: parse(&self.status).unwrap_or(ScheduleStatus::Cancelled),
            created_by: self.created_by.clone(),
            created_at: time(self.created_at),
            updated_at: time(self.updated_at),
            content: self.content(),
        }
    }
}

/// 按标识取出这个账户的一条日程；不是这个账户的，与不存在同样回答。
pub async fn load(
    executor: impl SqliteExecutor<'_, Database = sqlx::Sqlite>,
    account_id: &str,
    id: &str,
) -> Result<ScheduleRow, ApiError> {
    sqlx::query_as::<_, ScheduleRow>("SELECT * FROM schedules WHERE account_id = ? AND id = ?")
        .bind(account_id)
        .bind(id)
        .fetch_optional(executor)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn insert(
    executor: impl SqliteExecutor<'_, Database = sqlx::Sqlite>,
    id: &str,
    identity: &Identity,
    content: &ScheduleContent,
    now: i64,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO schedules \
         (id, account_id, title, location_label, location_address, start_at, end_at, repeat, \
          repeat_until, remind_before, notes, status, version, created_by, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'upcoming', 1, ?, ?, ?)",
    )
    .bind(id)
    .bind(&identity.account_id)
    .bind(&content.title)
    .bind(content.location.as_ref().map(|l| l.label.as_str()))
    .bind(content.location.as_ref().map(|l| l.address.as_str()))
    .bind(content.start_at.as_millis())
    .bind(content.end_at.map(Timestamp::as_millis))
    .bind(spelling(&content.repeat))
    .bind(content.repeat_until.map(Timestamp::as_millis))
    .bind(content.remind_before.map(i64::from))
    .bind(&content.notes)
    .bind(&identity.device_name)
    .bind(now)
    .bind(now)
    .execute(executor)
    .await?;
    Ok(())
}

/// 整份替换内容，版本号加一。
async fn write(
    executor: impl SqliteExecutor<'_, Database = sqlx::Sqlite>,
    id: &str,
    content: &ScheduleContent,
    version: i64,
    now: i64,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE schedules SET title = ?, location_label = ?, location_address = ?, start_at = ?, \
         end_at = ?, repeat = ?, repeat_until = ?, remind_before = ?, notes = ?, version = ?, \
         updated_at = ? WHERE id = ?",
    )
    .bind(&content.title)
    .bind(content.location.as_ref().map(|l| l.label.as_str()))
    .bind(content.location.as_ref().map(|l| l.address.as_str()))
    .bind(content.start_at.as_millis())
    .bind(content.end_at.map(Timestamp::as_millis))
    .bind(spelling(&content.repeat))
    .bind(content.repeat_until.map(Timestamp::as_millis))
    .bind(content.remind_before.map(i64::from))
    .bind(&content.notes)
    .bind(version)
    .bind(now)
    .bind(id)
    .execute(executor)
    .await?;
    Ok(())
}

/// 时间与重复规则有没有变。变了才需要清点旧的通知。
fn same_moments(before: &ScheduleContent, after: &ScheduleContent) -> bool {
    before.start_at == after.start_at
        && before.end_at == after.end_at
        && before.repeat == after.repeat
        && before.repeat_until == after.repeat_until
        && before.remind_before == after.remind_before
}

/// `POST /v1/schedules`：新建一条日程。带了 `Idempotency-Key` 而且这个键用过，拿回原来那一条。
pub async fn create(
    State(state): State<Shared>,
    identity: Identity,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let input: ScheduleContent = http::read_json(&body)?;
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
    insert(&mut *tx, &id, &identity, &content, now).await?;
    if let Some(key) = &key {
        remember(&mut *tx, &identity.account_id, key, RESOURCE, &id).await?;
    }
    tx.commit().await?;
    let row = load(&state.pool, &identity.account_id, &id).await?;
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        actions::SCHEDULE_CREATED,
        Some(&row.id),
    )
    .await;
    publish(&state.hub, &state.pool, &identity.account_id).await;
    Ok((StatusCode::CREATED, Json(row.view())).into_response())
}

/// `PUT /v1/schedules/{id}`：整份替换内容。版本号对不上、或日程已经结束，都被拒绝。
pub async fn update(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Schedule>, ApiError> {
    let input: ScheduleUpdate = http::read_json(&body)?;
    let Some(content) = input.content.cleaned() else {
        return Err(not_acceptable(RESOURCE));
    };
    let now = Timestamp::now().as_millis();
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let row = load(&mut *tx, &identity.account_id, &id).await?;
    if row.version != input.version {
        return Err(version_conflict());
    }
    if row.status != "upcoming" {
        return Err(invalid_state(
            "a finished or cancelled schedule cannot be edited; reopen it first",
        ));
    }
    let before = row.content();
    if same_moments(&before, &content) {
        notifications::refresh(&mut *tx, &row.id, &content).await?;
    } else {
        notifications::drop_stale(&mut tx, &row.id, &content, now).await?;
    }
    write(&mut *tx, &row.id, &content, row.version + 1, now).await?;
    tx.commit().await?;

    let updated = load(&state.pool, &identity.account_id, &row.id).await?;
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        actions::SCHEDULE_UPDATED,
        Some(&updated.id),
    )
    .await;
    publish(&state.hub, &state.pool, &identity.account_id).await;
    Ok(Json(updated.view()))
}

/// 改状态。版本号对不上就被拒绝；已经是目标状态时什么都不做。
async fn change_status(
    state: &Shared,
    identity: &Identity,
    id: &str,
    input: Versioned,
    target: ScheduleStatus,
    action: &'static str,
) -> Result<Schedule, ApiError> {
    let now = Timestamp::now().as_millis();
    let wanted = spelling(&target);
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let row = load(&mut *tx, &identity.account_id, id).await?;
    if row.version != input.version {
        return Err(version_conflict());
    }
    if row.status != wanted {
        // 完成、取消之后，还没读的提醒不再补发给任何设备。
        if target != ScheduleStatus::Upcoming {
            notifications::invalidate_for_schedule(&mut *tx, &row.id, now).await?;
        }
        sqlx::query(
            "UPDATE schedules SET status = ?, version = version + 1, updated_at = ? WHERE id = ?",
        )
        .bind(wanted)
        .bind(now)
        .bind(&row.id)
        .execute(&mut *tx)
        .await?;
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

/// `POST /v1/schedules/{id}/complete`：整条日程标成已完成。
pub async fn complete(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Schedule>, ApiError> {
    change_status(
        &state,
        &identity,
        &id,
        versioned(&body)?,
        ScheduleStatus::Done,
        actions::SCHEDULE_COMPLETED,
    )
    .await
    .map(Json)
}

/// `POST /v1/schedules/{id}/cancel`：取消。
pub async fn cancel(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Schedule>, ApiError> {
    change_status(
        &state,
        &identity,
        &id,
        versioned(&body)?,
        ScheduleStatus::Cancelled,
        actions::SCHEDULE_CANCELLED,
    )
    .await
    .map(Json)
}

/// `POST /v1/schedules/{id}/reopen`：完成或取消之后改回进行中。
pub async fn reopen(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Schedule>, ApiError> {
    change_status(
        &state,
        &identity,
        &id,
        versioned(&body)?,
        ScheduleStatus::Upcoming,
        actions::SCHEDULE_REOPENED,
    )
    .await
    .map(Json)
}

/// `DELETE /v1/schedules/{id}`：连它的通知一起删掉。
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
    sqlx::query("DELETE FROM notifications WHERE schedule_id = ?")
        .bind(&row.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM schedules WHERE id = ?")
        .bind(&row.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        actions::SCHEDULE_DELETED,
        Some(&row.id),
    )
    .await;
    publish(&state.hub, &state.pool, &identity.account_id).await;
    Ok(StatusCode::NO_CONTENT)
}
