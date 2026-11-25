//! 发送到设备：把一个文件经服务端送给同一个账户的另一台在线设备。
//!
//! 一次发送的状态只往前走：
//!
//! ```text
//! offered ──▶ delivered   接收方收下并确认
//!        ├──▶ declined    接收方拒绝
//!        ├──▶ cancelled   发送方撤回
//!        ├──▶ failed      接收方写盘失败
//!        └──▶ expired     过了时限还没送达（见 `maintenance`）
//! ```
//!
//! 两条规矩：
//!
//! - **只能发给在线设备。** 发起时对方必须在线；上传途中它下线了，这次发送也不成立。
//!   服务端不替离线设备保管文件。
//! - **到达终态就释放内容。** 除了 `offered`，其余状态下文件都不在服务端了。
//!   所以接收方必须先把文件存好，再来确认。

use std::sync::Arc;

use assemble_protocol::drive::{clean_file_name, is_mime_type};
use assemble_protocol::error::codes;
use assemble_protocol::events::Event;
use assemble_protocol::primitives::{MAX_FILE_SIZE, Timestamp, is_sha256_hex, is_valid_id};
// 协议里的"一次发送"叫 `Send`，会遮住 `std::marker::Send`；这里换个名字引进来。
use assemble_protocol::sends::{
    Direction, FailInput, FailReason, Send as SendRecord, SendListing, SendQuery, SendState,
};
use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::TryStreamExt;
use serde::Deserialize;
use sqlx::SqlitePool;
use tokio_util::io::StreamReader;
use tokio_util::sync::CancellationToken;

use crate::audit::{self, actions};
use crate::auth::Identity;
use crate::blobs::{self, StoreError};
use crate::error::ApiError;
use crate::events::Message;
use crate::http::{self, Shared};

/// 一次发送在库里的样子。设备名是发送当时的快照：设备之后被删掉、改名，记录仍然读得懂。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SendRow {
    pub id: String,
    pub account_id: String,
    pub from_device_id: String,
    pub from_device_name: String,
    pub to_device_id: String,
    pub to_device_name: String,
    pub name: String,
    pub size: i64,
    pub mime_type: Option<String>,
    pub sha256: String,
    pub state: String,
    pub failure: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

fn state_name(state: SendState) -> &'static str {
    match state {
        SendState::Offered => "offered",
        SendState::Delivered => "delivered",
        SendState::Declined => "declined",
        SendState::Cancelled => "cancelled",
        SendState::Failed => "failed",
        SendState::Expired => "expired",
    }
}

fn state_from(stored: &str) -> Option<SendState> {
    [
        SendState::Offered,
        SendState::Delivered,
        SendState::Declined,
        SendState::Cancelled,
        SendState::Failed,
        SendState::Expired,
    ]
    .into_iter()
    .find(|state| state_name(*state) == stored)
}

fn failure_name(reason: FailReason) -> &'static str {
    match reason {
        FailReason::DiskFull => "disk_full",
        FailReason::WriteFailed => "write_failed",
        FailReason::Cancelled => "cancelled",
        FailReason::VerificationFailed => "verification_failed",
    }
}

fn failure_from(stored: Option<&str>) -> Option<FailReason> {
    let stored = stored?;
    [
        FailReason::DiskFull,
        FailReason::WriteFailed,
        FailReason::Cancelled,
        FailReason::VerificationFailed,
    ]
    .into_iter()
    .find(|reason| failure_name(*reason) == stored)
}

fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap_or_else(Timestamp::now)
}

impl SendRow {
    /// 这份记录交给 `viewer` 这台设备看的样子：方向与对端都按它算。
    pub fn view_for(&self, viewer: &str) -> Result<SendRecord, ApiError> {
        let Some(state) = state_from(&self.state) else {
            tracing::error!(state = %self.state, "unknown send state stored in the database");
            return Err(ApiError::internal("a stored send has an unknown state"));
        };
        let incoming = self.to_device_id == viewer;
        Ok(SendRecord {
            id: self.id.clone(),
            direction: if incoming {
                Direction::In
            } else {
                Direction::Out
            },
            peer_device_id: if incoming {
                self.from_device_id.clone()
            } else {
                self.to_device_id.clone()
            },
            peer_device_name: if incoming {
                self.from_device_name.clone()
            } else {
                self.to_device_name.clone()
            },
            name: self.name.clone(),
            size: u64::try_from(self.size).unwrap_or(u64::MAX),
            mime_type: self.mime_type.clone(),
            sha256: self.sha256.clone(),
            state,
            failure: failure_from(self.failure.as_deref()),
            created_at: at(self.created_at),
            updated_at: at(self.updated_at),
        })
    }
}

/// 发起一次发送时的参数，放在查询串里；正文就是文件的字节。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSend {
    to_device_id: String,
    name: String,
    size: u64,
    sha256: String,
    #[serde(default)]
    mime_type: Option<String>,
}

fn conflict(code: &'static str, message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, code, message)
}

fn rejected(error: &StoreError) -> ApiError {
    let (code, status) = error.rejection();
    if matches!(error, StoreError::Io(_)) {
        tracing::warn!(%error, "an upload could not be stored");
    }
    ApiError::new(
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        code,
        match error {
            StoreError::TooLarge => "the file exceeds the single-file size limit",
            StoreError::Mismatch => "the content does not match the declared digest or size",
            StoreError::InvalidDigest => "the declared digest is not a SHA-256 hex string",
            StoreError::Stalled => "the upload stopped producing data",
            StoreError::Cancelled => "the upload was cancelled",
            StoreError::Io(_) => "the content could not be stored",
        },
    )
}

/// 核对发起发送的参数与请求头。返回去掉两端空白后的文件名。
fn check_new_send(input: &NewSend, headers: &axum::http::HeaderMap) -> Result<String, ApiError> {
    let name = clean_file_name(&input.name)
        .ok_or_else(|| ApiError::invalid("the file name is not acceptable"))?
        .to_owned();
    if !is_valid_id(&input.to_device_id) || !is_sha256_hex(&input.sha256) {
        return Err(ApiError::invalid("the send request is not valid"));
    }
    if input
        .mime_type
        .as_deref()
        .is_some_and(|mime| !is_mime_type(mime))
    {
        return Err(ApiError::invalid("the media type is not acceptable"));
    }
    // 请求头已经说明正文有多大：不必读下去就能拒绝。
    let announced = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if input.size > MAX_FILE_SIZE || announced.is_some_and(|length| length > MAX_FILE_SIZE) {
        return Err(too_large());
    }
    Ok(name)
}

fn too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        codes::FILE_TOO_LARGE,
        "the file exceeds the single-file size limit",
    )
}

fn target_offline() -> ApiError {
    conflict(codes::DEVICE_OFFLINE, "the target device is offline")
}

/// 服务端的内容总量到了上限。和"云盘空间不够"回答同一个码：对发文件的一方来说下一步一样。
fn storage_full() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        codes::QUOTA_EXCEEDED,
        "the server has no room for this file",
    )
}

fn is_online(state: &Shared, account_id: &str, device_id: &str) -> bool {
    state.presence.is_online(account_id, device_id)
}

/// `POST /v1/sends?toDeviceId=&name=&size=&sha256=&mimeType=`
pub async fn create(
    State(state): State<Shared>,
    identity: Identity,
    query: Result<Query<NewSend>, QueryRejection>,
    request: Request,
) -> Result<Response, ApiError> {
    let Query(input) =
        query.map_err(|_| ApiError::invalid("the query string is not a valid send request"))?;
    let (parts, body) = request.into_parts();
    let name = check_new_send(&input, &parts.headers);

    // 这几项检查不用看正文。拒绝之前要让对方把话说完，见 [`crate::http::refuse_upload`]。
    let admitted = match (
        name,
        crate::devices::find(&state.pool, &identity.account_id, &input.to_device_id).await?,
    ) {
        (Ok(name), Some(target)) if target.id != identity.device_id => {
            // 只能发给在线设备：离线的设备没人收，服务端也不替它保管。
            if !is_online(&state, &identity.account_id, &target.id) {
                Err(target_offline())
            } else if blobs::over_budget(
                &state.pool,
                &input.sha256,
                input.size,
                state.config.storage_budget_bytes,
            )
            .await?
            {
                // 全站的总量先粗筛一遍，免得白收；作数的是入库时事务里的那一次。
                Err(storage_full())
            } else {
                Ok((target, name))
            }
        }
        (Ok(_), Some(_)) => Err(ApiError::invalid("a device cannot send to itself")),
        (Ok(_), None) => Err(ApiError::not_found()),
        (Err(error), _) => Err(error),
    };
    let (target, name) = match admitted {
        Ok(admitted) => admitted,
        Err(error) => return Err(crate::http::refuse_upload(body, error).await),
    };

    // 正文里的传输错误在进入存放逻辑之前统一成 I/O 错误。
    let stream = body.into_data_stream().map_err(std::io::Error::other);
    let mut reader = StreamReader::new(stream);
    let cancel = CancellationToken::new();
    state
        .blobs
        .store(&mut reader, input.size, &input.sha256, &cancel)
        .await
        .map_err(|error| rejected(&error))?;

    // 上传可能持续了一阵子。对方这会儿已经下线的话，这次发送同样不成立。
    if !is_online(&state, &identity.account_id, &target.id) {
        state.blobs.discard_unreferenced(&input.sha256).await?;
        return Err(target_offline());
    }

    let row = match record_send(&state, &identity, &target, &input, name).await {
        Ok(row) => row,
        Err(error) => {
            // 没记成发送，刚收下的内容就没人引用：别把它留在磁盘上等着清扫。
            // 真的记上了的话，这里会看见它还被引用着，什么都不删。
            state.blobs.discard_unreferenced(&input.sha256).await?;
            return Err(error);
        }
    };
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        actions::SEND_CREATED,
        Some(&row.id),
    )
    .await;
    // 告诉接收方：有文件到了。只有它收得到这条事件。
    state.hub.publish(
        &identity.account_id,
        Message::Event {
            audience: Some(target.id.clone()),
            event: Arc::new(Event::SendOffered {
                send: row.view_for(&target.id)?,
            }),
        },
    );
    tracing::info!(send = %row.id, size = input.size, "send offered");

    Ok((
        StatusCode::CREATED,
        Json(row.view_for(&identity.device_id)?),
    )
        .into_response())
}

/// 给内容加引用、记下这次发送：要么都发生，要么都不。
async fn record_send(
    state: &Shared,
    identity: &Identity,
    target: &crate::devices::DeviceRow,
    input: &NewSend,
    name: String,
) -> Result<SendRow, ApiError> {
    let id = crate::secrets::random_id().map_err(|error| ApiError::internal(error.to_string()))?;
    let size = i64::try_from(input.size).map_err(|_| ApiError::invalid("the file is too large"))?;
    let now = Timestamp::now().as_millis();
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    if blobs::over_budget(
        &mut *tx,
        &input.sha256,
        input.size,
        state.config.storage_budget_bytes,
    )
    .await?
    {
        tx.rollback().await?;
        tracing::warn!("the storage budget is reached: a send was refused");
        return Err(storage_full());
    }
    blobs::retain(&mut *tx, &input.sha256, input.size).await?;
    sqlx::query(
        "INSERT INTO sends \
         (id, account_id, from_device_id, from_device_name, to_device_id, to_device_name, \
          name, size, mime_type, sha256, state, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'offered', ?, ?)",
    )
    .bind(&id)
    .bind(&identity.account_id)
    .bind(&identity.device_id)
    .bind(&identity.device_name)
    .bind(&target.id)
    .bind(&target.name)
    .bind(&name)
    .bind(size)
    .bind(&input.mime_type)
    .bind(&input.sha256)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(SendRow {
        id,
        account_id: identity.account_id.clone(),
        from_device_id: identity.device_id.clone(),
        from_device_name: identity.device_name.clone(),
        to_device_id: target.id.clone(),
        to_device_name: target.name.clone(),
        name,
        size,
        mime_type: input.mime_type.clone(),
        sha256: input.sha256.clone(),
        state: state_name(SendState::Offered).to_owned(),
        failure: None,
        created_at: now,
        updated_at: now,
    })
}

fn send_query(query: Result<Query<SendQuery>, QueryRejection>) -> Result<SendQuery, ApiError> {
    let Query(query) = query
        .map_err(|_| ApiError::invalid("the query string is not a valid send listing request"))?;
    if !query.is_valid() {
        return Err(ApiError::invalid("the send listing request is not valid"));
    }
    Ok(query)
}

/// `GET /v1/sends?direction=in|out&state=&limit=`
pub async fn list(
    State(state): State<Shared>,
    identity: Identity,
    query: Result<Query<SendQuery>, QueryRejection>,
) -> Result<Json<SendListing>, ApiError> {
    let query = send_query(query)?;
    // `in` 是别人发给这台设备的；`out` 是这台设备发给别人的。
    // 查询由几段固定的文字拼成，取值一律走绑定参数。
    let mut sql = sqlx::QueryBuilder::new(if query.direction == Direction::In {
        "SELECT * FROM sends WHERE account_id = ? AND to_device_id = ?"
    } else {
        "SELECT * FROM sends WHERE account_id = ? AND to_device_id != ?"
    });
    if query.state.is_some() {
        sql.push(" AND state = ?");
    }
    // 同一毫秒里的记录按写入先后倒着排。
    sql.push(" ORDER BY created_at DESC, rowid DESC LIMIT ?");

    let mut request = sql
        .build_query_as::<SendRow>()
        .bind(&identity.account_id)
        .bind(&identity.device_id);
    if let Some(wanted) = query.state {
        request = request.bind(state_name(wanted));
    }
    let rows = request
        .bind(i64::from(query.limit))
        .fetch_all(&state.pool)
        .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(row.view_for(&identity.device_id)?);
    }
    Ok(Json(SendListing { items }))
}

async fn load(pool: &SqlitePool, account_id: &str, id: &str) -> Result<SendRow, ApiError> {
    sqlx::query_as::<_, SendRow>("SELECT * FROM sends WHERE account_id = ? AND id = ?")
        .bind(account_id)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(ApiError::not_found)
}

/// `GET /v1/sends/{id}/content`：只有接收方能取，而且只在 `offered` 期间。
pub async fn content(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    let row = load(&state.pool, &identity.account_id, &id).await?;
    if row.to_device_id != identity.device_id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            codes::FORBIDDEN,
            "only the receiving device can download this send",
        ));
    }
    if state_from(&row.state) != Some(SendState::Offered) {
        return Err(conflict(
            codes::SEND_STATE_CONFLICT,
            "the content of this send is no longer available",
        ));
    }
    Ok(http::file_response(
        &state.blobs.path_for(&row.sha256),
        &row.name,
        row.mime_type.as_deref(),
        request.headers(),
    )
    .await)
}

/// 谁在对这次发送做决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Sender,
    Receiver,
}

/// 把一次 `offered` 的发送推到终态，释放内容，并通知另一边。
///
/// 已经处于目标状态时原样返回当前记录：网络抖动导致的重复回执不算错误。
/// 处于别的终态时拒绝。
async fn settle(
    state: &Shared,
    identity: &Identity,
    id: &str,
    to: SendState,
    failure: Option<FailReason>,
    side: Side,
    action: &'static str,
) -> Result<SendRecord, ApiError> {
    let mut row = load(&state.pool, &identity.account_id, id).await?;
    let allowed = match side {
        Side::Sender => row.from_device_id == identity.device_id,
        Side::Receiver => row.to_device_id == identity.device_id,
    };
    if !allowed {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            codes::FORBIDDEN,
            "this device cannot settle that send",
        ));
    }
    let now = Timestamp::now().as_millis();
    let written = sqlx::query(
        "UPDATE sends SET state = ?, failure = ?, updated_at = ? \
         WHERE id = ? AND state = 'offered'",
    )
    .bind(state_name(to))
    .bind(failure.map(failure_name))
    .bind(now)
    .bind(id)
    .execute(&state.pool)
    .await?;
    if written.rows_affected() == 0 {
        if state_from(&row.state) == Some(to) {
            return row.view_for(&identity.device_id);
        }
        return Err(conflict(
            codes::SEND_STATE_CONFLICT,
            "the send is no longer waiting to be settled",
        ));
    }
    // 内容到此为止：除了 offered，任何状态下服务端都不再保管它。
    state.blobs.release(&row.sha256).await?;
    state_name(to).clone_into(&mut row.state);
    row.failure = failure.map(|reason| failure_name(reason).to_owned());
    row.updated_at = now;

    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        action,
        Some(id),
    )
    .await;
    let other = match side {
        Side::Sender => row.to_device_id.clone(),
        Side::Receiver => row.from_device_id.clone(),
    };
    state.hub.publish(
        &identity.account_id,
        Message::Event {
            audience: Some(other.clone()),
            event: Arc::new(Event::SendUpdated {
                send: row.view_for(&other)?,
            }),
        },
    );
    tracing::info!(send = %id, state = state_name(to), "send settled");
    row.view_for(&identity.device_id)
}

/// `POST /v1/sends/{id}/accept`
pub async fn accept(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<SendRecord>, ApiError> {
    settle(
        &state,
        &identity,
        &id,
        SendState::Delivered,
        None,
        Side::Receiver,
        actions::SEND_ACCEPTED,
    )
    .await
    .map(Json)
}

/// `POST /v1/sends/{id}/decline`
pub async fn decline(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<SendRecord>, ApiError> {
    settle(
        &state,
        &identity,
        &id,
        SendState::Declined,
        None,
        Side::Receiver,
        actions::SEND_DECLINED,
    )
    .await
    .map(Json)
}

/// `POST /v1/sends/{id}/cancel`
pub async fn cancel(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<SendRecord>, ApiError> {
    settle(
        &state,
        &identity,
        &id,
        SendState::Cancelled,
        None,
        Side::Sender,
        actions::SEND_CANCELLED,
    )
    .await
    .map(Json)
}

/// `POST /v1/sends/{id}/failed`
pub async fn failed(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<SendRecord>, ApiError> {
    let input: FailInput = crate::http::read_json(&body)?;
    settle(
        &state,
        &identity,
        &id,
        SendState::Failed,
        Some(input.reason),
        Side::Receiver,
        actions::SEND_FAILED,
    )
    .await
    .map(Json)
}

/// 给定时清理用：把一条过期的 `offered` 发送标成 `expired` 并释放内容。
/// 返回最新的记录，以及该通知谁。
pub(crate) async fn expire(state: &Shared, row: &SendRow) -> Result<Option<SendRow>, sqlx::Error> {
    let now = Timestamp::now().as_millis();
    let written = sqlx::query(
        "UPDATE sends SET state = 'expired', updated_at = ? WHERE id = ? AND state = 'offered'",
    )
    .bind(now)
    .bind(&row.id)
    .execute(&state.pool)
    .await?;
    if written.rows_affected() == 0 {
        return Ok(None);
    }
    let _ = state.blobs.release(&row.sha256).await;
    let mut expired = row.clone();
    state_name(SendState::Expired).clone_into(&mut expired.state);
    expired.updated_at = now;
    audit::record(
        &state.pool,
        Some(&row.account_id),
        None,
        actions::SEND_EXPIRED,
        Some(&row.id),
    )
    .await;
    Ok(Some(expired))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn row() -> SendRow {
        SendRow {
            id: "s1".into(),
            account_id: "a1".into(),
            from_device_id: "pc".into(),
            from_device_name: "工作电脑".into(),
            to_device_id: "mac".into(),
            to_device_name: "MacBook".into(),
            name: "合同.pdf".into(),
            size: 1024,
            mime_type: Some("application/pdf".into()),
            sha256: "a".repeat(64),
            state: "offered".into(),
            failure: None,
            created_at: 1_794_400_000_000,
            updated_at: 1_794_400_000_000,
        }
    }

    #[test]
    fn stored_names_agree_with_the_protocol_spelling() {
        for state in [
            SendState::Offered,
            SendState::Delivered,
            SendState::Declined,
            SendState::Cancelled,
            SendState::Failed,
            SendState::Expired,
        ] {
            let wire = serde_json::to_value(state).unwrap();
            assert_eq!(json!(state_name(state)), wire, "{state:?}");
            assert_eq!(state_from(state_name(state)), Some(state));
        }
        assert_eq!(state_from("unknown"), None);
        for reason in [
            FailReason::DiskFull,
            FailReason::WriteFailed,
            FailReason::Cancelled,
            FailReason::VerificationFailed,
        ] {
            assert_eq!(
                json!(failure_name(reason)),
                serde_json::to_value(reason).unwrap()
            );
            assert_eq!(failure_from(Some(failure_name(reason))), Some(reason));
        }
        assert_eq!(failure_from(None), None);
        assert_eq!(failure_from(Some("mystery")), None);
    }

    #[test]
    fn the_direction_and_peer_depend_on_who_is_looking() {
        let row = row();
        let seen_by_receiver = row.view_for("mac").unwrap();
        assert_eq!(seen_by_receiver.direction, Direction::In);
        assert_eq!(seen_by_receiver.peer_device_name, "工作电脑");
        assert_eq!(seen_by_receiver.peer_device_id, "pc");

        let seen_by_sender = row.view_for("pc").unwrap();
        assert_eq!(seen_by_sender.direction, Direction::Out);
        assert_eq!(seen_by_sender.peer_device_name, "MacBook");
        // 两边看到的是同一次发送的其余信息。
        assert_eq!(seen_by_sender.name, seen_by_receiver.name);
        assert_eq!(seen_by_sender.sha256, seen_by_receiver.sha256);
        assert!(seen_by_sender.content_available());

        let mut settled = row.clone();
        settled.state = "failed".into();
        settled.failure = Some("disk_full".into());
        let failed = settled.view_for("pc").unwrap();
        assert_eq!(failed.state, SendState::Failed);
        assert_eq!(failed.failure, Some(FailReason::DiskFull));
        assert!(!failed.content_available());
    }

    #[test]
    fn an_unknown_stored_state_is_an_error_not_a_guess() {
        let mut row = row();
        row.state = "lost".into();
        assert!(row.view_for("pc").is_err());
    }

    #[test]
    fn content_disposition_encodes_everything_but_safe_ascii() {
        assert_eq!(
            http::disposition("合同 v2.pdf"),
            "attachment; filename*=UTF-8''%E5%90%88%E5%90%8C%20v2.pdf"
        );
        assert_eq!(
            http::disposition("a-b_c.d~e"),
            "attachment; filename*=UTF-8''a-b_c.d~e"
        );
        assert_eq!(http::disposition(""), "attachment; filename*=UTF-8''");
    }
}
