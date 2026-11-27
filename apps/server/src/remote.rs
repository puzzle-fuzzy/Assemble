//! 同账户远程桌面中转。内存会话、最新一帧和有界输入；重启即失效。
use crate::{auth::Identity, error::ApiError, http::Shared};
use assemble_protocol::primitives::Timestamp;
use assemble_protocol::remote::{
    MAX_BATCH, MAX_FRAME_BASE64, MAX_FRAME_BYTES, MAX_INPUTS, PEER_TIMEOUT_MS, REQUEST_TTL_MS,
    RemoteBatch, RemoteFrame, RemotePhase, RemotePoll, RemoteRequest, RemoteSession, RemoteView,
    SequencedInput,
};
use assemble_protocol::remote::{RemoteEnd, RemoteEndReason};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, PoisonError};

#[derive(Default)]
pub struct RemoteSessions {
    entries: Mutex<HashMap<String, Entry>>,
}
impl std::fmt::Debug for RemoteSessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteSessions").finish_non_exhaustive()
    }
}
struct Entry {
    account: String,
    session: RemoteSession,
    controller_seen: i64,
    host_seen: i64,
    frame: Option<RemoteFrame>,
    inputs: VecDeque<SequencedInput>,
    input_sequence: u32,
    frame_at: i64,
}
impl Entry {
    fn end(&mut self, reason: RemoteEndReason) {
        if self.session.phase == RemotePhase::Ended {
            return;
        }
        self.session.phase = RemotePhase::Ended;
        self.session.end_reason = Some(reason);
        self.frame = None;
        self.inputs.clear();
    }
    fn expire(&mut self, now: i64) {
        let expired = match self.session.phase {
            RemotePhase::Pending => now - self.session.created_at.as_millis() > REQUEST_TTL_MS,
            RemotePhase::Active => {
                now - self.controller_seen > PEER_TIMEOUT_MS
                    || now - self.host_seen > PEER_TIMEOUT_MS
            }
            RemotePhase::Ended => false,
        };
        if expired {
            self.end(if self.session.phase == RemotePhase::Pending {
                RemoteEndReason::Expired
            } else {
                RemoteEndReason::Disconnected
            });
        }
    }
    fn includes(&self, id: &Identity) -> bool {
        self.account == id.account_id
            && (self.session.host_id == id.device_id || self.session.controller_id == id.device_id)
    }
    fn active(&self) -> Result<(), ApiError> {
        if self.session.phase == RemotePhase::Active {
            Ok(())
        } else {
            Err(ApiError::conflict(
                "REMOTE_ENDED",
                "remote session is not active",
            ))
        }
    }
}
impl RemoteSessions {
    pub fn prune(&self) {
        let now = Timestamp::now().as_millis();
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        for entry in entries.values_mut() {
            entry.expire(now);
        }
        entries.retain(|_, entry| {
            entry.session.phase != RemotePhase::Ended
                || now - entry.controller_seen.max(entry.host_seen) < REQUEST_TTL_MS
        });
    }
}
pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/v1/remote/sessions", get(list).post(create))
        .route("/v1/remote/sessions/{id}/accept", post(accept))
        .route("/v1/remote/sessions/{id}/end", post(end))
        .route("/v1/remote/sessions/{id}/poll", get(poll))
        .route("/v1/remote/sessions/{id}/frame", post(frame))
        .route("/v1/remote/sessions/{id}/inputs", post(inputs))
        .layer(DefaultBodyLimit::max(MAX_FRAME_BASE64 + 32 * 1024))
}
fn no_store(value: impl serde::Serialize) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response()
}

async fn list(State(state): State<Shared>, identity: Identity) -> Result<Response, ApiError> {
    state.remote.prune();
    let sessions = state
        .remote
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .values()
        .filter(|e| e.includes(&identity) && e.session.phase != RemotePhase::Ended)
        .map(|e| e.session.clone())
        .collect();
    Ok(no_store(RemoteView { sessions }))
}
async fn create(
    State(state): State<Shared>,
    identity: Identity,
    Json(input): Json<RemoteRequest>,
) -> Result<Response, ApiError> {
    let host = crate::devices::find(&state.pool, &identity.account_id, &input.host_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let controller = crate::devices::find(&state.pool, &identity.account_id, &identity.device_id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    if host.id == identity.device_id || !desktop(&host.platform) || !desktop(&controller.platform) {
        return Err(ApiError::invalid("two desktop devices are required"));
    }
    if !state.presence.is_online(&identity.account_id, &host.id) {
        return Err(ApiError::conflict("DEVICE_OFFLINE", "host is offline"));
    }
    state.remote.prune();
    let now = Timestamp::now();
    let session = RemoteSession {
        id: crate::secrets::random_id().map_err(|_| ApiError::internal("id unavailable"))?,
        controller_id: identity.device_id.clone(),
        controller_name: identity.device_name.clone(),
        host_id: host.id,
        host_name: host.name,
        phase: RemotePhase::Pending,
        created_at: now,
        end_reason: None,
    };
    {
        let mut entries = state
            .remote
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // 重试返回原请求，不延长确认时限。任一电脑同时最多参加一个会话，避免控制回路。
        if let Some(existing) = entries.values().find(|e| {
            e.account == identity.account_id
                && e.session.phase != RemotePhase::Ended
                && e.session.controller_id == identity.device_id
                && e.session.host_id == input.host_id
        }) {
            return Ok(no_store(existing.session.clone()));
        }
        if entries.len() >= 128
            || entries.values().any(|e| {
                e.session.phase != RemotePhase::Ended
                    && [&e.session.host_id, &e.session.controller_id]
                        .iter()
                        .any(|id| **id == session.host_id || **id == session.controller_id)
            })
        {
            return Err(ApiError::conflict(
                "REMOTE_BUSY",
                "a device already participates in a session",
            ));
        }
        entries.insert(
            session.id.clone(),
            Entry {
                account: identity.account_id.clone(),
                session: session.clone(),
                controller_seen: now.as_millis(),
                host_seen: now.as_millis(),
                frame: None,
                inputs: VecDeque::new(),
                input_sequence: 0,
                frame_at: 0,
            },
        );
    }
    crate::audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        "remote.requested",
        Some(&session.id),
    )
    .await;
    Ok(no_store(session))
}
fn desktop(platform: &str) -> bool {
    matches!(platform, "windows" | "macos")
}

/// 每次数据请求同时核验两端仍存在；撤销设备与停用账户不能留下可用的数据通道。
async fn authorize(state: &Shared, identity: &Identity, id: &str) -> Result<(), ApiError> {
    let peer = {
        let mut entries = state
            .remote
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let e = entries
            .get_mut(id)
            .filter(|e| e.includes(identity))
            .ok_or_else(ApiError::not_found)?;
        e.expire(Timestamp::now().as_millis());
        if e.session.host_id == identity.device_id {
            e.session.controller_id.clone()
        } else {
            e.session.host_id.clone()
        }
    };
    if crate::devices::find(&state.pool, &identity.account_id, &peer)
        .await?
        .is_none()
        && let Some(e) = state
            .remote
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(id)
    {
        e.end(RemoteEndReason::DeviceRevoked);
    }
    Ok(())
}
async fn accept(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    authorize(&state, &identity, &id).await?;
    let session = {
        let mut entries = state
            .remote
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let e = entries.get_mut(&id).ok_or_else(ApiError::not_found)?;
        if e.session.host_id != identity.device_id {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "FORBIDDEN",
                "only the host may approve",
            ));
        }
        if e.session.phase == RemotePhase::Ended {
            return Err(ApiError::conflict("REMOTE_ENDED", "request expired"));
        }
        if e.session.phase == RemotePhase::Pending {
            e.session.phase = RemotePhase::Active;
            let now = Timestamp::now().as_millis();
            e.controller_seen = now;
            e.host_seen = now;
        }
        e.session.clone()
    };
    crate::audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        "remote.accepted",
        Some(&id),
    )
    .await;
    Ok(no_store(session))
}
async fn end(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<StatusCode, ApiError> {
    authorize(&state, &identity, &id).await?;
    let reason = if body.is_empty() {
        RemoteEndReason::Stopped
    } else {
        if body.len() > 256 {
            return Err(ApiError::invalid("invalid remote end reason"));
        }
        serde_json::from_slice::<RemoteEnd>(&body)
            .map_err(|_| ApiError::invalid("invalid remote end reason"))?
            .reason
    };
    {
        let mut entries = state
            .remote
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let e = entries.get_mut(&id).ok_or_else(ApiError::not_found)?;
        // 服务端决定拒绝与过期；设备只报告自己停止、断网或原生能力失败。
        if !matches!(
            reason,
            RemoteEndReason::Stopped
                | RemoteEndReason::Disconnected
                | RemoteEndReason::CaptureFailed
                | RemoteEndReason::InputFailed
        ) {
            return Err(ApiError::invalid("end reason is server-owned"));
        }
        if e.session.host_id != identity.device_id
            && matches!(
                reason,
                RemoteEndReason::CaptureFailed | RemoteEndReason::InputFailed
            )
        {
            return Err(ApiError::invalid("only the host may report native errors"));
        }
        let reason = if e.session.phase == RemotePhase::Pending
            && e.session.host_id == identity.device_id
            && reason == RemoteEndReason::Stopped
        {
            RemoteEndReason::Declined
        } else {
            reason
        };
        e.end(reason);
    }
    crate::audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        "remote.ended",
        Some(&id),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PollQuery {
    #[serde(default)]
    after: u32,
    #[serde(default)]
    frame_after: u32,
}
async fn poll(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    Query(query): Query<PollQuery>,
) -> Result<Response, ApiError> {
    authorize(&state, &identity, &id).await?;
    let mut entries = state
        .remote
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let e = entries.get_mut(&id).ok_or_else(ApiError::not_found)?;
    let host = e.session.host_id == identity.device_id;
    let now = Timestamp::now().as_millis();
    if host {
        e.host_seen = now;
    } else {
        e.controller_seen = now;
    }
    let mut result = RemotePoll {
        session: e.session.clone(),
        frame: None,
        inputs: Vec::new(),
    };
    if e.session.phase == RemotePhase::Active {
        if host {
            if query.after > e.input_sequence {
                return Err(ApiError::invalid("invalid input acknowledgement"));
            }
            e.inputs.retain(|event| event.sequence > query.after);
            result.inputs = e.inputs.iter().take(MAX_BATCH).cloned().collect();
        } else {
            result.frame = e
                .frame
                .as_ref()
                .filter(|f| f.sequence > query.frame_after)
                .cloned();
        }
    }
    Ok(no_store(result))
}
async fn frame(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    Json(frame): Json<RemoteFrame>,
) -> Result<StatusCode, ApiError> {
    authorize(&state, &identity, &id).await?;
    if !frame.valid() {
        return Err(ApiError::invalid("invalid remote frame"));
    }
    let bytes = STANDARD
        .decode(&frame.jpeg)
        .map_err(|_| ApiError::invalid("invalid jpeg encoding"))?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(ApiError::invalid("frame exceeds limit"));
    }
    let size =
        image::ImageReader::with_format(std::io::Cursor::new(&bytes), image::ImageFormat::Jpeg)
            .into_dimensions()
            .map_err(|_| ApiError::invalid("invalid jpeg"))?;
    if size != (frame.width, frame.height) {
        return Err(ApiError::invalid("frame dimensions disagree"));
    }
    let mut entries = state
        .remote
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let e = entries.get_mut(&id).ok_or_else(ApiError::not_found)?;
    e.active()?;
    if e.session.host_id != identity.device_id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "only the host may send frames",
        ));
    }
    if e.frame
        .as_ref()
        .is_some_and(|last| frame.sequence <= last.sequence)
    {
        return Ok(StatusCode::NO_CONTENT);
    }
    let now = Timestamp::now().as_millis();
    if now - e.frame_at < 60 {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "REMOTE_RATE_LIMITED",
            "frame rate exceeded",
        ));
    }
    e.frame_at = now;
    e.frame = Some(frame);
    Ok(StatusCode::NO_CONTENT)
}
async fn inputs(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    Json(batch): Json<RemoteBatch>,
) -> Result<StatusCode, ApiError> {
    authorize(&state, &identity, &id).await?;
    if batch.events.is_empty()
        || batch.events.len() > MAX_BATCH
        || batch.events.iter().any(|event| event.sequence == 0)
        || batch
            .events
            .windows(2)
            .any(|pair| pair[0].sequence.checked_add(1) != Some(pair[1].sequence))
        || batch.events.iter().any(|event| !event.input.valid())
    {
        return Err(ApiError::invalid("invalid remote inputs"));
    }
    let mut entries = state
        .remote
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let e = entries.get_mut(&id).ok_or_else(ApiError::not_found)?;
    e.active()?;
    if e.session.controller_id != identity.device_id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "only the controller may send inputs",
        ));
    }
    let mut next = e.input_sequence;
    for event in &batch.events {
        if event.sequence > next {
            if next.checked_add(1) != Some(event.sequence) {
                return Err(ApiError::invalid("input sequence gap"));
            }
            next = event.sequence;
        }
    }
    let count = batch
        .events
        .iter()
        .filter(|event| event.sequence > e.input_sequence)
        .count();
    if e.inputs.len() + count > MAX_INPUTS {
        e.end(RemoteEndReason::InputFailed);
        return Err(ApiError::conflict("REMOTE_ENDED", "input queue overflow"));
    }
    for event in batch.events {
        if event.sequence > e.input_sequence {
            e.input_sequence = event.sequence;
            e.inputs.push_back(event);
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(now: i64) -> Entry {
        Entry {
            account: "account".into(),
            session: RemoteSession {
                id: "session".into(),
                controller_id: "controller".into(),
                controller_name: "Windows".into(),
                host_id: "host".into(),
                host_name: "Mac".into(),
                phase: RemotePhase::Pending,
                created_at: Timestamp::from_millis(now).unwrap(),
                end_reason: None,
            },
            controller_seen: now,
            host_seen: now,
            frame: None,
            inputs: VecDeque::from([SequencedInput {
                sequence: 1,
                input: assemble_protocol::remote::RemoteInput::ReleaseAll,
            }]),
            input_sequence: 1,
            frame_at: 0,
        }
    }
    #[test]
    fn pending_requests_cannot_be_kept_alive_forever_and_active_sessions_need_both_peers() {
        let now = 1_800_000_000_000;
        let mut pending = entry(now);
        pending.controller_seen = now + REQUEST_TTL_MS;
        pending.host_seen = now + REQUEST_TTL_MS;
        pending.expire(now + REQUEST_TTL_MS + 1);
        assert_eq!(pending.session.phase, RemotePhase::Ended);
        assert_eq!(pending.session.end_reason, Some(RemoteEndReason::Expired));
        assert!(pending.inputs.is_empty());
        let mut active = entry(now);
        active.session.phase = RemotePhase::Active;
        active.controller_seen = now + PEER_TIMEOUT_MS + 1;
        active.expire(now + PEER_TIMEOUT_MS + 1);
        assert_eq!(active.session.phase, RemotePhase::Ended);
        assert_eq!(
            active.session.end_reason,
            Some(RemoteEndReason::Disconnected)
        );
        assert!(active.inputs.is_empty());
    }
}
