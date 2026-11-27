//! 远程桌面只走固定来源的 Rust 请求管线，设备凭据从不交给画面窗口。
use crate::{Client, ClientError, RequestOptions};
use assemble_protocol::remote::secure::{
    Channel, WireAudio, WireAudioPacket, WireBatch, WireClipboardResult, WireFrame,
};
use assemble_protocol::remote::{ClipboardRequest, ClipboardResult};
use assemble_protocol::remote::{
    RemoteBatch, RemoteFrame, RemotePoll, RemoteRequest, RemoteSession, RemoteView,
};
use assemble_protocol::remote::{RemoteEnd, RemoteEndReason};
use reqwest::Method;
use serde::{Serialize, de::DeserializeOwned};

pub(crate) fn path(id: &str, action: &str) -> Result<String, ClientError> {
    if !assemble_protocol::primitives::is_valid_id(id) || matches!(id, "." | "..") {
        return Err(ClientError::InvalidRequest);
    }
    Ok(format!("/v1/remote/sessions/{id}/{action}"))
}
impl Client {
    pub async fn remote_audio_upload(
        &self,
        id: &str,
        audio: &assemble_protocol::remote::RemoteAudio,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        if !audio.valid() {
            return Err(ClientError::InvalidRequest);
        }
        if audio.packets.iter().any(|packet| {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(&packet.data)
                .map_or(true, |bytes| bytes.is_empty() || bytes.len() > 1275)
        }) {
            return Err(ClientError::InvalidRequest);
        }
        let state = self
            .remote_security
            .seal(id, Channel::AudioState, 0, &audio.state)?;
        let packets = audio
            .packets
            .iter()
            .map(|packet| {
                Ok(WireAudioPacket {
                    sequence: packet.sequence,
                    packet: self.remote_security.seal(
                        id,
                        Channel::AudioPacket,
                        packet.sequence,
                        packet,
                    )?,
                })
            })
            .collect::<Result<Vec<_>, ClientError>>()?;
        let wire = WireAudio {
            state: Some(state),
            packets,
        };
        self.remote_call(
            Method::POST,
            &path(id, "audio")?,
            vec![],
            Some(payload(&wire)?),
            options,
        )
        .await?;
        Ok(())
    }
    pub async fn remote_audio_poll(
        &self,
        id: &str,
        after: u32,
        options: &RequestOptions<'_>,
    ) -> Result<assemble_protocol::remote::RemoteAudio, ClientError> {
        let wire: WireAudio = self
            .remote_json(
                Method::GET,
                &path(id, "audio")?,
                vec![("after", after.to_string())],
                None,
                options,
            )
            .await?;
        if wire.packets.len() > assemble_protocol::remote::MAX_AUDIO_PACKETS {
            return Err(ClientError::InvalidResponse);
        }
        let state = wire
            .state
            .as_ref()
            .map(|packet| {
                self.remote_security
                    .open(id, packet, Channel::AudioState, 0)
            })
            .transpose()?
            .unwrap_or_default();
        let mut packets = Vec::new();
        for packet in wire.packets {
            let decoded: assemble_protocol::remote::AudioPacket = self.remote_security.open(
                id,
                &packet.packet,
                Channel::AudioPacket,
                packet.sequence,
            )?;
            if decoded.sequence != packet.sequence || packet.sequence <= after {
                return Err(ClientError::InvalidResponse);
            }
            packets.push(decoded);
        }
        let audio = assemble_protocol::remote::RemoteAudio { state, packets };
        if !audio.valid() {
            return Err(ClientError::InvalidResponse);
        }
        Ok(audio)
    }
    pub async fn remote_clipboard_exchange(
        &self,
        id: &str,
        operation: assemble_protocol::remote::ClipboardOperation,
        options: &RequestOptions<'_>,
    ) -> Result<ClipboardResult, ClientError> {
        use assemble_protocol::remote::ClipboardStatus;
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(|_| ClientError::InvalidRequest)?;
        let request = ClipboardRequest {
            id: assemble_protocol::primitives::to_hex(&random),
            operation,
        };
        self.remote_clipboard_request(id, &request, options).await?;
        let wait = async {
            loop {
                let result = self
                    .remote_clipboard_status(id, &request.id, options)
                    .await?;
                if result.status != ClipboardStatus::Pending {
                    return Ok(result);
                }
                tokio::select! { biased;
                    () = options.cancel.cancelled() => return Err(ClientError::Cancelled),
                    () = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(12), wait)
            .await
            .unwrap_or({
                Ok(ClipboardResult {
                    id: request.id,
                    status: ClipboardStatus::Expired,
                    text: None,
                })
            })
    }
    pub async fn remote_clipboard_request(
        &self,
        id: &str,
        request: &ClipboardRequest,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        if !request.valid() {
            return Err(ClientError::InvalidRequest);
        }
        if !self.remote_security.ready(id) {
            let _: assemble_protocol::remote::secure::Handshake = self
                .remote_json(Method::GET, &path(id, "handshake")?, vec![], None, options)
                .await?;
            return Err(ClientError::InvalidRequest);
        }
        let wire = self.remote_security.clipboard(id, request)?;
        self.remote_call(
            Method::POST,
            &path(id, "clipboard")?,
            vec![],
            Some(payload(&wire)?),
            options,
        )
        .await?;
        Ok(())
    }
    pub async fn remote_clipboard_result(
        &self,
        id: &str,
        result: &ClipboardResult,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        if !result.valid() {
            return Err(ClientError::InvalidRequest);
        }
        self.remote_security.check_clipboard(id, result)?;
        let wire = WireClipboardResult {
            id: result.id.clone(),
            expired: false,
            packet: Some(
                self.remote_security
                    .seal(id, Channel::ClipboardResult, 0, result)?,
            ),
        };
        self.remote_call(
            Method::POST,
            &path(id, "clipboard/result")?,
            vec![],
            Some(payload(&wire)?),
            options,
        )
        .await?;
        Ok(())
    }
    pub async fn remote_clipboard_status(
        &self,
        id: &str,
        request_id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<ClipboardResult, ClientError> {
        if !assemble_protocol::primitives::is_valid_id(request_id) {
            return Err(ClientError::InvalidRequest);
        }
        let action = format!("clipboard/{request_id}");
        let wire: WireClipboardResult = self
            .remote_json(Method::GET, &path(id, &action)?, vec![], None, options)
            .await?;
        if wire.id != request_id {
            return Err(ClientError::InvalidResponse);
        }
        let result: ClipboardResult = match wire.packet {
            Some(packet) => self
                .remote_security
                .open(id, &packet, Channel::ClipboardResult, 0)?,
            None => ClipboardResult {
                id: wire.id,
                status: if wire.expired {
                    assemble_protocol::remote::ClipboardStatus::Expired
                } else {
                    assemble_protocol::remote::ClipboardStatus::Pending
                },
                text: None,
            },
        };
        if result.id != request_id || !result.valid() {
            return Err(ClientError::InvalidResponse);
        }
        self.remote_security.check_clipboard(id, &result)?;
        Ok(result)
    }
    pub(crate) async fn remote_call(
        &self,
        method: Method,
        path: &str,
        query: Vec<(&'static str, String)>,
        body: Option<serde_json::Value>,
        options: &RequestOptions<'_>,
    ) -> Result<reqwest::Response, ClientError> {
        if !self.has_bearer() {
            return Err(ClientError::InvalidRequest);
        }
        self.json_request(method, path, query, body, options).await
    }
    pub(crate) async fn remote_json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: Vec<(&'static str, String)>,
        body: Option<serde_json::Value>,
        options: &RequestOptions<'_>,
    ) -> Result<T, ClientError> {
        let response = self.remote_call(method, path, query, body, options).await?;
        let bytes = crate::media::bounded_body(
            response,
            assemble_protocol::remote::secure::MAX_SEALED_BASE64
                * assemble_protocol::remote::MAX_VIDEO_FRAMES
                + 64 * 1024,
            options,
        )
        .await?;
        serde_json::from_slice(&bytes).map_err(|_| ClientError::InvalidResponse)
    }
    pub async fn remote_sessions(
        &self,
        options: &RequestOptions<'_>,
    ) -> Result<RemoteView, ClientError> {
        let mut view: RemoteView = self
            .remote_json(Method::GET, "/v1/remote/sessions", vec![], None, options)
            .await?;
        self.remote_security.prune();
        for session in &mut view.sessions {
            self.remote_security.decorate(session);
        }
        Ok(view)
    }
    pub async fn remote_request(
        &self,
        input: &RemoteRequest,
        options: &RequestOptions<'_>,
    ) -> Result<RemoteSession, ClientError> {
        self.secure_request(&input.host_id, options).await
    }
    pub async fn remote_accept(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<RemoteSession, ClientError> {
        if !self.remote_security.live(id) {
            return Err(ClientError::InvalidRequest);
        }
        let mut session: RemoteSession = self
            .remote_json(Method::POST, &path(id, "accept")?, vec![], None, options)
            .await?;
        self.remote_security.decorate(&mut session);
        Ok(session)
    }
    pub async fn remote_end(
        &self,
        id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        let ended = self
            .remote_call(Method::POST, &path(id, "end")?, vec![], None, options)
            .await;
        self.remote_security.remove(id);
        ended?;
        Ok(())
    }
    pub async fn remote_end_with_reason(
        &self,
        id: &str,
        reason: RemoteEndReason,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        let ended = self
            .remote_call(
                Method::POST,
                &path(id, "end")?,
                vec![],
                Some(payload(&RemoteEnd { reason })?),
                options,
            )
            .await;
        self.remote_security.remove(id);
        ended?;
        Ok(())
    }
    pub async fn remote_poll(
        &self,
        id: &str,
        after: u32,
        frame_after: u32,
        options: &RequestOptions<'_>,
    ) -> Result<RemotePoll, ClientError> {
        self.secure_poll(id, after, frame_after, options).await
    }
    pub async fn remote_frame(
        &self,
        id: &str,
        frame: &RemoteFrame,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        if !frame.valid() {
            return Err(ClientError::InvalidRequest);
        }
        let wire = WireFrame {
            sequence: frame.sequence,
            packet: self
                .remote_security
                .seal(id, Channel::Frame, frame.sequence, frame)?,
        };
        self.remote_call(
            Method::POST,
            &path(id, "frame")?,
            vec![],
            Some(payload(&wire)?),
            options,
        )
        .await?;
        Ok(())
    }
    pub async fn remote_inputs(
        &self,
        id: &str,
        batch: &RemoteBatch,
        options: &RequestOptions<'_>,
    ) -> Result<(), ClientError> {
        if batch.events.is_empty()
            || batch.events.len() > assemble_protocol::remote::MAX_BATCH
            || batch.events.iter().any(|e| !e.input.valid())
        {
            return Err(ClientError::InvalidRequest);
        }
        let wire = WireBatch {
            events: batch
                .events
                .iter()
                .map(|event| self.remote_security.input(id, event))
                .collect::<Result<Vec<_>, _>>()?,
        };
        self.remote_call(
            Method::POST,
            &path(id, "inputs")?,
            vec![],
            Some(payload(&wire)?),
            options,
        )
        .await?;
        Ok(())
    }
}
pub(crate) fn payload(value: &impl Serialize) -> Result<serde_json::Value, ClientError> {
    serde_json::to_value(value).map_err(|_| ClientError::InvalidRequest)
}
