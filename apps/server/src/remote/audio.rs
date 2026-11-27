//! 音频只保留小段有界密文，不能从中转读取声音或播放状态。
use super::{
    ApiError, Identity, Json, Path, PoisonError, Query, Response, Shared, State, StatusCode,
    authorize, no_store, valid_packet,
};
use assemble_protocol::remote::{MAX_AUDIO_PACKETS, secure::WireAudio};
pub(super) async fn upload(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    Json(audio): Json<WireAudio>,
) -> Result<StatusCode, ApiError> {
    authorize(&state, &identity, &id).await?;
    if audio
        .state
        .as_ref()
        .is_none_or(|packet| !valid_packet(packet, 4096))
        || audio.packets.len() > MAX_AUDIO_PACKETS
        || audio
            .packets
            .iter()
            .any(|packet| packet.sequence == 0 || !valid_packet(&packet.packet, 8192))
        || audio
            .packets
            .windows(2)
            .any(|pair| pair[0].sequence >= pair[1].sequence)
    {
        return Err(ApiError::invalid("invalid encrypted audio"));
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
            "only the host may upload audio",
        ));
    }
    entry.audio.state = audio.state;
    if audio.packets.is_empty() {
        entry.audio.packets.clear();
    }
    for packet in audio.packets {
        if packet.sequence > entry.audio_sequence {
            entry.audio_sequence = packet.sequence;
            entry.audio.packets.push(packet);
        }
    }
    if entry.audio.packets.len() > MAX_AUDIO_PACKETS {
        entry
            .audio
            .packets
            .drain(..entry.audio.packets.len() - MAX_AUDIO_PACKETS);
    }
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Default, serde::Deserialize)]
pub(super) struct After {
    #[serde(default)]
    after: u32,
}
pub(super) async fn poll(
    State(state): State<Shared>,
    identity: Identity,
    Path(id): Path<String>,
    Query(query): Query<After>,
) -> Result<Response, ApiError> {
    authorize(&state, &identity, &id).await?;
    let entries = state
        .remote
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let entry = entries.get(&id).ok_or_else(ApiError::not_found)?;
    entry.active()?;
    if entry.session.controller_id != identity.device_id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "only the controller may read audio",
        ));
    }
    Ok(no_store(WireAudio {
        state: entry.audio.state.clone(),
        packets: entry
            .audio
            .packets
            .iter()
            .filter(|packet| packet.sequence > query.after)
            .cloned()
            .collect(),
    }))
}
