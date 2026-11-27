//! 服务端仅保留当前剪贴板请求的密文；期限或结束后立即清空。
use super::{
    ApiError, Entry, Identity, Json, Path, PoisonError, Response, Shared, State, StatusCode,
    Timestamp, authorize, no_store, valid_packet,
};
use assemble_protocol::remote::{
    CLIPBOARD_TIMEOUT_MS,
    secure::{WireClipboardRequest, WireClipboardResult, WireClipboardTask},
};
pub(super) struct Exchange {
    pub task: WireClipboardTask,
    pub result: WireClipboardResult,
}
impl Exchange {
    pub fn expire(&mut self, now: i64) {
        if now >= self.task.request.expires_at.as_millis() {
            self.task.request.packet.chunks.clear();
            self.result.packet = None;
            self.result.expired = true;
        }
    }
}
fn controller(entry: &Entry, identity: &Identity) -> Result<(), ApiError> {
    if entry.session.controller_id != identity.device_id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "only the controller may initiate clipboard exchange",
        ));
    }
    Ok(())
}
pub(super) async fn request(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    Json(mut request): Json<WireClipboardRequest>,
) -> Result<StatusCode, ApiError> {
    authorize(&state, &identity, &id).await?;
    if !assemble_protocol::primitives::is_valid_id(&request.id)
        || !valid_packet(&request.packet, 512 * 1024)
    {
        return Err(ApiError::invalid("invalid encrypted clipboard request"));
    }
    let mut entries = state
        .remote
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let entry = entries.get_mut(&id).ok_or_else(ApiError::not_found)?;
    entry.active()?;
    controller(entry, &identity)?;
    if let Some(old) = &entry.clipboard {
        if old.task.request.id == request.id {
            return Ok(StatusCode::NO_CONTENT);
        }
        if old.result.packet.is_none() && !old.result.expired {
            return Err(ApiError::conflict(
                "REMOTE_CLIPBOARD_BUSY",
                "clipboard exchange pending",
            ));
        }
    }
    let limit = Timestamp::from_millis(Timestamp::now().as_millis() + CLIPBOARD_TIMEOUT_MS)
        .ok_or_else(|| ApiError::internal("invalid clipboard deadline"))?;
    request.expires_at = request.expires_at.min(limit);
    entry.clipboard = Some(Exchange {
        result: WireClipboardResult {
            id: request.id.clone(),
            expired: false,
            packet: None,
        },
        task: WireClipboardTask {
            request,
            after_input: entry.input_sequence,
        },
    });
    Ok(StatusCode::NO_CONTENT)
}
pub(super) async fn result(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    Json(result): Json<WireClipboardResult>,
) -> Result<StatusCode, ApiError> {
    authorize(&state, &identity, &id).await?;
    if result.expired
        || result
            .packet
            .as_ref()
            .is_none_or(|packet| !valid_packet(packet, 512 * 1024))
    {
        return Err(ApiError::invalid("invalid encrypted clipboard result"));
    }
    let mut entries = state
        .remote
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let entry = entries.get_mut(&id).ok_or_else(ApiError::not_found)?;
    entry.active()?;
    if entry.session.host_id != identity.device_id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "only the host may complete clipboard exchange",
        ));
    }
    let exchange = entry
        .clipboard
        .as_mut()
        .filter(|exchange| exchange.task.request.id == result.id)
        .ok_or_else(ApiError::not_found)?;
    if exchange.result.packet.is_none() && !exchange.result.expired {
        exchange.result = result;
        exchange.task.request.packet.chunks.clear();
    }
    Ok(StatusCode::NO_CONTENT)
}
pub(super) async fn status(
    State(state): State<Shared>,
    identity: Identity,
    Path((id, request)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    authorize(&state, &identity, &id).await?;
    let entries = state
        .remote
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let entry = entries.get(&id).ok_or_else(ApiError::not_found)?;
    entry.active()?;
    controller(entry, &identity)?;
    let exchange = entry
        .clipboard
        .as_ref()
        .filter(|exchange| exchange.task.request.id == request)
        .ok_or_else(ApiError::not_found)?;
    Ok(no_store(&exchange.result))
}
#[cfg(test)]
mod tests {
    use super::*;
    use assemble_protocol::remote::secure::Sealed;
    #[test]
    fn expiration_clears_both_directions() {
        let deadline = Timestamp::from_millis(1_800_000_000_000).unwrap();
        let encrypted = Sealed {
            nonce: 0,
            chunks: vec!["ciphertext".into()],
        };
        let mut exchange = Exchange {
            task: WireClipboardTask {
                request: WireClipboardRequest {
                    id: "clipboard".into(),
                    expires_at: deadline,
                    packet: encrypted.clone(),
                },
                after_input: 0,
            },
            result: WireClipboardResult {
                id: "clipboard".into(),
                expired: false,
                packet: Some(encrypted),
            },
        };
        exchange.expire(deadline.as_millis());
        assert!(exchange.task.request.packet.chunks.is_empty());
        assert!(exchange.result.expired);
        assert!(exchange.result.packet.is_none());
    }
}
