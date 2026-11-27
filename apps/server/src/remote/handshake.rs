//! 握手字节只做有界中转，摘要与身份核对由两端 Rust 完成。
use super::{
    ApiError, Identity, Json, Path, PoisonError, Response, Shared, State, StatusCode, authorize,
    no_store,
};
use assemble_protocol::remote::secure::HandshakeStep;
use base64::{Engine as _, engine::general_purpose::STANDARD};
pub(super) async fn read(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    authorize(&state, &identity, &id).await?;
    let entries = state
        .remote
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let entry = entries.get(&id).ok_or_else(ApiError::not_found)?;
    let mut handshake = entry.handshake.clone();
    handshake.session = entry.session.clone();
    Ok(no_store(handshake))
}
pub(super) async fn write(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<HandshakeStep>,
) -> Result<StatusCode, ApiError> {
    authorize(&state, &identity, &id).await?;
    if STANDARD
        .decode(&input.data)
        .map_or(true, |bytes| bytes.is_empty() || bytes.len() > 2048)
    {
        return Err(ApiError::invalid("invalid handshake packet"));
    }
    let mut entries = state
        .remote
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let entry = entries.get_mut(&id).ok_or_else(ApiError::not_found)?;
    if entry.session.phase == assemble_protocol::remote::RemotePhase::Ended {
        return Err(ApiError::conflict("REMOTE_ENDED", "remote session ended"));
    }
    let slot = match input.step {
        2 if identity.device_id == entry.session.host_id => &mut entry.handshake.response,
        3 if identity.device_id == entry.session.controller_id
            && entry.handshake.response.is_some() =>
        {
            &mut entry.handshake.finish
        }
        _ => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "FORBIDDEN",
                "handshake direction is invalid",
            ));
        }
    };
    if let Some(old) = slot {
        if old != &input.data {
            return Err(ApiError::invalid("handshake packet changed"));
        }
    } else {
        *slot = Some(input.data);
    }
    Ok(StatusCode::NO_CONTENT)
}
