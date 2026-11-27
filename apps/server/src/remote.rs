//! 同账户远程桌面中转。内存会话、最新一帧和有界输入；重启即失效。
use crate::{auth::Identity, error::ApiError, http::Shared};
use assemble_protocol::primitives::Timestamp;
use assemble_protocol::remote::secure::{
    Handshake, MAX_SEALED_BASE64, SECURE_VERSION, Sealed, WireAudio, WireBatch, WireCreate,
    WireFrame, WireInput, WirePoll, WirePollRequest,
};
use assemble_protocol::remote::{
    MAX_BATCH, MAX_INPUTS, PEER_TIMEOUT_MS, REQUEST_TTL_MS, RemoteEnd, RemoteEndReason,
    RemotePhase, RemoteSession, RemoteView,
};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, PoisonError};

mod audio;
mod clipboard;
mod handshake;

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
    frames: VecDeque<WireFrame>,
    inputs: VecDeque<WireInput>,
    input_sequence: u32,
    frame_at: i64,
    clipboard: Option<clipboard::Exchange>,
    audio: WireAudio,
    audio_sequence: u32,
    handshake: Handshake,
    host_keepalive: Option<Sealed>,
    controller_keepalive: Option<Sealed>,
}
impl Entry {
    fn frame_bytes(&self) -> usize {
        self.frames.iter().map(|frame| frame.packet.bytes()).sum()
    }
    fn end(&mut self, reason: RemoteEndReason) {
        if self.session.phase == RemotePhase::Ended {
            return;
        }
        self.session.phase = RemotePhase::Ended;
        self.session.end_reason = Some(reason);
        self.frames.clear();
        self.inputs.clear();
        self.clipboard = None;
        self.audio = WireAudio::default();
        self.host_keepalive = None;
        self.controller_keepalive = None;
        self.handshake.offer.clear();
        self.handshake.response = None;
        self.handshake.finish = None;
    }
    fn expire(&mut self, now: i64) {
        if let Some(exchange) = &mut self.clipboard {
            exchange.expire(now);
        }
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
        .route("/v1/remote/sessions/{id}/poll", post(poll))
        .route(
            "/v1/remote/sessions/{id}/handshake",
            get(handshake::read).post(handshake::write),
        )
        .route("/v1/remote/sessions/{id}/frame", post(frame))
        .route("/v1/remote/sessions/{id}/inputs", post(inputs))
        .route(
            "/v1/remote/sessions/{id}/audio",
            get(audio::poll).post(audio::upload),
        )
        .route(
            "/v1/remote/sessions/{id}/clipboard",
            post(clipboard::request),
        )
        .route(
            "/v1/remote/sessions/{id}/clipboard/result",
            post(clipboard::result),
        )
        .route(
            "/v1/remote/sessions/{id}/clipboard/{request}",
            get(clipboard::status),
        )
        .layer(DefaultBodyLimit::max(MAX_SEALED_BASE64 * 2))
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
    Json(input): Json<WireCreate>,
) -> Result<Response, ApiError> {
    if input.version != SECURE_VERSION
        || STANDARD
            .decode(&input.offer)
            .map_or(true, |bytes| bytes.len() < 32 || bytes.len() > 2048)
    {
        return Err(ApiError::invalid("invalid secure remote offer"));
    }
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
        verification: None,
    };
    {
        let mut entries = state
            .remote
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(existing) = entries.values_mut().find(|entry| {
            entry.account == identity.account_id
                && entry.session.phase != RemotePhase::Ended
                && entry.session.controller_id == identity.device_id
                && entry.session.host_id == input.host_id
        }) {
            if existing.handshake.offer == input.offer {
                return Ok(no_store(existing.session.clone()));
            }
            if existing.session.phase == RemotePhase::Pending {
                existing.end(RemoteEndReason::Stopped);
            }
        }
        if entries.len() >= 128
            || entries.values().any(|entry| {
                entry.session.phase != RemotePhase::Ended
                    && [&entry.session.host_id, &entry.session.controller_id]
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
                frames: VecDeque::new(),
                inputs: VecDeque::new(),
                input_sequence: 0,
                frame_at: 0,
                clipboard: None,
                audio: WireAudio::default(),
                audio_sequence: 0,
                handshake: Handshake {
                    session: session.clone(),
                    offer: input.offer,
                    response: None,
                    finish: None,
                },
                host_keepalive: None,
                controller_keepalive: None,
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
            if e.handshake.finish.is_none() {
                return Err(ApiError::conflict(
                    "REMOTE_SECURITY_PENDING",
                    "secure handshake is incomplete",
                ));
            }
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
fn valid_packet(packet: &Sealed, max: usize) -> bool {
    packet.valid()
        && packet.bytes() <= max
        && packet.chunks.iter().all(|chunk| {
            STANDARD
                .decode(chunk)
                .is_ok_and(|bytes| (16..=61_456).contains(&bytes.len()))
        })
}
async fn poll(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    Json(query): Json<WirePollRequest>,
) -> Result<Response, ApiError> {
    authorize(&state, &identity, &id).await?;
    if !valid_packet(&query.keepalive, 4096) {
        return Err(ApiError::invalid("invalid secure keepalive"));
    }
    let mut entries = state
        .remote
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let entry = entries.get_mut(&id).ok_or_else(ApiError::not_found)?;
    let host = entry.session.host_id == identity.device_id;
    let mut result = WirePoll {
        session: entry.session.clone(),
        frames: Vec::new(),
        inputs: Vec::new(),
        clipboard: None,
        keepalive: None,
    };
    if entry.session.phase == RemotePhase::Ended {
        return Ok(no_store(result));
    }
    if entry.handshake.finish.is_none() {
        return Err(ApiError::conflict(
            "REMOTE_SECURITY_PENDING",
            "secure handshake is incomplete",
        ));
    }
    let now = Timestamp::now().as_millis();
    if host {
        entry.host_seen = now;
        entry.host_keepalive = Some(query.keepalive);
        result.keepalive.clone_from(&entry.controller_keepalive);
    } else {
        entry.controller_seen = now;
        entry.controller_keepalive = Some(query.keepalive);
        result.keepalive.clone_from(&entry.host_keepalive);
    }
    if entry.session.phase == RemotePhase::Active {
        if host {
            if query.after > entry.input_sequence {
                return Err(ApiError::invalid("invalid input acknowledgement"));
            }
            entry.inputs.retain(|event| event.sequence > query.after);
            result.inputs = entry.inputs.iter().take(MAX_BATCH).cloned().collect();
            result.clipboard = entry
                .clipboard
                .as_ref()
                .filter(|exchange| {
                    exchange.result.packet.is_none()
                        && !exchange.result.expired
                        && exchange.task.after_input <= query.after
                })
                .map(|exchange| exchange.task.clone());
        } else {
            result.frames = entry
                .frames
                .iter()
                .filter(|frame| frame.sequence > query.frame_after)
                .cloned()
                .collect();
        }
    }
    Ok(no_store(result))
}
async fn frame(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    Json(frame): Json<WireFrame>,
) -> Result<StatusCode, ApiError> {
    authorize(&state, &identity, &id).await?;
    if frame.sequence == 0 || !valid_packet(&frame.packet, MAX_SEALED_BASE64) {
        return Err(ApiError::invalid("invalid encrypted frame"));
    }
    let mut entries = state
        .remote
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let available = (32usize * 1024 * 1024).saturating_sub(
        entries
            .iter()
            .filter(|(key, _)| **key != id)
            .map(|(_, entry)| entry.frame_bytes())
            .sum::<usize>(),
    );
    if frame.packet.bytes() > available {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "REMOTE_BUSY",
            "remote frame budget exhausted",
        ));
    }
    let entry = entries.get_mut(&id).ok_or_else(ApiError::not_found)?;
    entry.active()?;
    if entry.session.host_id != identity.device_id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "only the host may send frames",
        ));
    }
    if entry
        .frames
        .back()
        .is_some_and(|last| frame.sequence <= last.sequence)
    {
        return Ok(StatusCode::NO_CONTENT);
    }
    let now = Timestamp::now().as_millis();
    if now - entry.frame_at < 20 {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "REMOTE_RATE_LIMITED",
            "frame rate exceeded",
        ));
    }
    entry.frame_at = now;
    entry.frames.push_back(frame);
    while entry.frames.len() > assemble_protocol::remote::MAX_VIDEO_FRAMES
        || entry.frame_bytes() > available.min(MAX_SEALED_BASE64 * 4)
    {
        entry.frames.pop_front();
    }
    Ok(StatusCode::NO_CONTENT)
}
async fn inputs(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    Json(batch): Json<WireBatch>,
) -> Result<StatusCode, ApiError> {
    authorize(&state, &identity, &id).await?;
    if batch.events.is_empty()
        || batch.events.len() > MAX_BATCH
        || batch
            .events
            .iter()
            .any(|event| event.sequence == 0 || !valid_packet(&event.packet, 48 * 1024))
        || batch
            .events
            .windows(2)
            .any(|pair| pair[0].sequence.checked_add(1) != Some(pair[1].sequence))
    {
        return Err(ApiError::invalid("invalid encrypted inputs"));
    }
    let mut entries = state
        .remote
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let buffered: usize = entries
        .values()
        .flat_map(|entry| &entry.inputs)
        .map(|event| event.packet.bytes())
        .sum();
    let entry = entries.get_mut(&id).ok_or_else(ApiError::not_found)?;
    entry.active()?;
    if entry.session.controller_id != identity.device_id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "only the controller may send inputs",
        ));
    }
    let mut next = entry.input_sequence;
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
        .filter(|event| event.sequence > entry.input_sequence)
        .count();
    let incoming: usize = batch
        .events
        .iter()
        .filter(|event| event.sequence > entry.input_sequence)
        .map(|event| event.packet.bytes())
        .sum();
    if entry.inputs.len() + count > MAX_INPUTS || buffered + incoming > 16 * 1024 * 1024 {
        entry.end(RemoteEndReason::InputFailed);
        return Err(ApiError::conflict("REMOTE_ENDED", "input queue overflow"));
    }
    for event in batch.events {
        if event.sequence > entry.input_sequence {
            entry.input_sequence = event.sequence;
            entry.inputs.push_back(event);
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(now: i64) -> Entry {
        let session = RemoteSession {
            id: "session".into(),
            controller_id: "controller".into(),
            controller_name: "Windows".into(),
            host_id: "host".into(),
            host_name: "Mac".into(),
            phase: RemotePhase::Pending,
            created_at: Timestamp::from_millis(now).unwrap(),
            end_reason: None,
            verification: None,
        };
        Entry {
            account: "account".into(),
            session: session.clone(),
            controller_seen: now,
            host_seen: now,
            frames: VecDeque::new(),
            inputs: VecDeque::from([WireInput {
                sequence: 1,
                packet: Sealed {
                    nonce: 0,
                    chunks: vec!["AA==".into()],
                },
            }]),
            input_sequence: 1,
            frame_at: 0,
            clipboard: None,
            audio: WireAudio::default(),
            audio_sequence: 0,
            handshake: Handshake {
                session,
                offer: String::new(),
                response: None,
                finish: None,
            },
            host_keepalive: None,
            controller_keepalive: None,
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
