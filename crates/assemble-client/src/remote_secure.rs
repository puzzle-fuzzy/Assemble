//! 握手与密钥留在 Rust。新鲜随机挑战的回显才证明对端在线，HTTP 成功不能续命。
use crate::{
    Client, ClientError, RequestOptions,
    remote::{path, payload},
};
use assemble_protocol::{
    primitives::Timestamp,
    remote::{
        CLIPBOARD_TIMEOUT_MS, ClipboardOperation, ClipboardRequest, ClipboardResult,
        ClipboardStatus, ClipboardTask, MAX_BATCH, MAX_VIDEO_FRAMES, RemoteFrame, RemotePhase,
        RemotePoll, RemoteSession, SequencedInput,
        secure::{
            Binding, Channel, Handshake, HandshakeStep, SECURE_VERSION, Sealed,
            WireClipboardRequest, WireCreate, WireInput, WirePoll, WirePollRequest,
        },
    },
};
use assemble_remote::crypto::{Cipher, Initiator, Responder};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::Method;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};
#[cfg(test)]
mod tests;

type Result<T> = std::result::Result<T, ClientError>;
enum Stage {
    Initiator(Initiator),
    Responder(Responder, String),
    Ready(Cipher, Option<String>),
}
struct Context {
    session: RemoteSession,
    stage: Option<Stage>,
    host: bool,
    challenge: String,
    issued: Instant,
    history: VecDeque<(String, Instant)>,
    answer: Option<String>,
    peer_counter: u32,
    sent_counter: u32,
    peer_seen: Instant,
    clipboard: Option<(String, bool)>,
    last_input: u32,
    cursor: Option<assemble_protocol::remote::CursorUpdate>,
    peer_cursor: Option<assemble_protocol::remote::CursorUpdate>,
    created: Instant,
}
#[derive(Clone, Default)]
pub(crate) struct Sessions(Arc<Mutex<HashMap<String, Context>>>);
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Heartbeat {
    challenge: String,
    answer: Option<String>,
    counter: u32,
    cursor: Option<assemble_protocol::remote::CursorUpdate>,
}
fn random() -> Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| ClientError::InvalidRequest)?;
    Ok(assemble_protocol::primitives::to_hex(&bytes))
}
fn binding(session: &RemoteSession) -> Binding {
    Binding {
        session_id: session.id.clone(),
        controller_id: session.controller_id.clone(),
        host_id: session.host_id.clone(),
    }
}
fn validate_frame(frame: &RemoteFrame) -> Result<()> {
    if !frame.valid() {
        return Err(ClientError::InvalidResponse);
    }
    if frame.video.is_none() {
        let jpeg = STANDARD
            .decode(&frame.jpeg)
            .map_err(|_| ClientError::InvalidResponse)?;
        let dimensions =
            image::ImageReader::with_format(std::io::Cursor::new(jpeg), image::ImageFormat::Jpeg)
                .into_dimensions()
                .map_err(|_| ClientError::InvalidResponse)?;
        if dimensions != (frame.width, frame.height) {
            return Err(ClientError::InvalidResponse);
        }
    }
    Ok(())
}
impl Context {
    fn new(session: RemoteSession, stage: Stage, host: bool) -> Result<Self> {
        Ok(Self {
            session,
            stage: Some(stage),
            host,
            challenge: random()?,
            issued: Instant::now(),
            history: VecDeque::new(),
            answer: None,
            peer_counter: 0,
            sent_counter: 0,
            peer_seen: Instant::now(),
            clipboard: None,
            last_input: 0,
            cursor: None,
            peer_cursor: None,
            created: Instant::now(),
        })
    }
    fn cipher(&self) -> Result<&Cipher> {
        match &self.stage {
            Some(Stage::Ready(cipher, _)) => Ok(cipher),
            _ => Err(ClientError::InvalidResponse),
        }
    }
    fn seal<T: Serialize>(&mut self, channel: Channel, sequence: u32, value: &T) -> Result<Sealed> {
        match &mut self.stage {
            Some(Stage::Ready(cipher, _)) => cipher
                .seal(channel, sequence, value)
                .map_err(|_| ClientError::InvalidResponse),
            _ => Err(ClientError::InvalidRequest),
        }
    }
    fn open<T: DeserializeOwned>(
        &self,
        packet: &Sealed,
        channel: Channel,
        sequence: u32,
    ) -> Result<T> {
        self.cipher()?
            .open(packet, channel, sequence)
            .map_err(|_| ClientError::InvalidResponse)
    }
    fn fresh_epoch(&self, challenge: &str) -> bool {
        (challenge == self.challenge && self.issued.elapsed() <= Duration::from_millis(5000))
            || self
                .history
                .iter()
                .any(|(old, at)| old == challenge && at.elapsed() <= Duration::from_millis(5000))
    }
    fn heartbeat(&mut self) -> Result<Sealed> {
        self.sent_counter = self
            .sent_counter
            .checked_add(1)
            .ok_or(ClientError::InvalidRequest)?;
        self.seal(
            Channel::Heartbeat,
            0,
            &Heartbeat {
                challenge: self.challenge.clone(),
                answer: self.answer.clone(),
                counter: self.sent_counter,
                cursor: self.cursor.clone(),
            },
        )
    }
    fn observe(&mut self, packet: &Sealed) -> Result<()> {
        let heartbeat: Heartbeat = self.open(packet, Channel::Heartbeat, 0)?;
        if heartbeat.challenge.len() != 32
            || !heartbeat
                .challenge
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(ClientError::InvalidResponse);
        }
        if heartbeat.counter <= self.peer_counter {
            return Ok(());
        }
        self.peer_counter = heartbeat.counter;
        if !self.host {
            self.peer_cursor = heartbeat.cursor;
        }
        self.answer = Some(heartbeat.challenge);
        if heartbeat.answer.as_deref() == Some(&self.challenge)
            && self.issued.elapsed() <= Duration::from_millis(5000)
        {
            self.history.push_back((
                std::mem::replace(&mut self.challenge, random()?),
                self.issued,
            ));
            self.issued = Instant::now();
            self.peer_seen = Instant::now();
        }
        while self
            .history
            .front()
            .is_some_and(|(_, at)| at.elapsed() > Duration::from_millis(5000))
            || self.history.len() > 128
        {
            self.history.pop_front();
        }
        Ok(())
    }
}
impl Sessions {
    pub fn prune(&self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, context| {
                context.session.phase != RemotePhase::Ended
                    && (context.session.phase != RemotePhase::Pending
                        || context.created.elapsed() <= Duration::from_secs(65))
            });
    }
    pub fn clear(&self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
    pub fn remove(&self, id: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
    }
    pub fn cursor(&self, id: &str, cursor: assemble_protocol::remote::CursorUpdate) -> Result<()> {
        self.with(id, |context| {
            if !context.host {
                return Err(ClientError::InvalidRequest);
            }
            context.cursor = Some(cursor);
            Ok(())
        })
    }
    fn with<T>(&self, id: &str, work: impl FnOnce(&mut Context) -> Result<T>) -> Result<T> {
        work(
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_mut(id)
                .ok_or(ClientError::InvalidRequest)?,
        )
    }
    pub fn decorate(&self, session: &mut RemoteSession) {
        session.verification = self
            .with(&session.id, |context| {
                Ok(context.cipher()?.code().to_owned())
            })
            .ok();
    }
    pub fn ready(&self, id: &str) -> bool {
        self.with(id, |context| context.cipher().map(|_| ()))
            .is_ok()
    }
    pub fn live(&self, id: &str) -> bool {
        self.with(id, |context| {
            Ok(context.cipher().is_ok()
                && context.peer_seen.elapsed() <= Duration::from_millis(5000))
        })
        .unwrap_or(false)
    }
    pub fn seal<T: Serialize>(
        &self,
        id: &str,
        channel: Channel,
        sequence: u32,
        value: &T,
    ) -> Result<Sealed> {
        self.with(id, |context| context.seal(channel, sequence, value))
    }
    pub fn open<T: DeserializeOwned>(
        &self,
        id: &str,
        packet: &Sealed,
        channel: Channel,
        sequence: u32,
    ) -> Result<T> {
        self.with(id, |context| context.open(packet, channel, sequence))
    }
    pub fn input(&self, id: &str, event: &SequencedInput) -> Result<WireInput> {
        self.with(id, |context| {
            context.last_input = context.last_input.max(event.sequence);
            let epoch = context.answer.clone().ok_or(ClientError::InvalidRequest)?;
            Ok(WireInput {
                sequence: event.sequence,
                packet: context.seal(Channel::Input, event.sequence, &(event, epoch))?,
            })
        })
    }
    pub fn clipboard(&self, id: &str, request: &ClipboardRequest) -> Result<WireClipboardRequest> {
        self.with(id, |context| {
            context.clipboard = Some((
                request.id.clone(),
                matches!(request.operation, ClipboardOperation::Read),
            ));
            let expires_at =
                Timestamp::from_millis(Timestamp::now().as_millis() + CLIPBOARD_TIMEOUT_MS)
                    .ok_or(ClientError::InvalidRequest)?;
            let epoch = context.answer.clone().ok_or(ClientError::InvalidRequest)?;
            let packet = context.seal(
                Channel::ClipboardRequest,
                0,
                &(request, expires_at, epoch, context.last_input),
            )?;
            Ok(WireClipboardRequest {
                id: request.id.clone(),
                expires_at,
                packet,
            })
        })
    }
    pub fn check_clipboard(&self, id: &str, result: &ClipboardResult) -> Result<()> {
        self.with(id, |context| {
            let Some((request, reading)) = &context.clipboard else {
                return Err(ClientError::InvalidResponse);
            };
            if request != &result.id
                || (result.status == ClipboardStatus::Completed
                    && *reading != result.text.is_some())
            {
                return Err(ClientError::InvalidResponse);
            }
            Ok(())
        })
    }
    fn poll(&self, id: &str, after: u32, frame_after: u32) -> Result<WirePollRequest> {
        self.with(id, |context| {
            Ok(WirePollRequest {
                after,
                frame_after,
                keepalive: context.heartbeat()?,
            })
        })
    }
    fn pending(&self, id: &str) -> Result<RemotePoll> {
        self.with(id, |context| {
            Ok(RemotePoll {
                cursor: None,
                session: context.session.clone(),
                frame: None,
                frames: Vec::new(),
                inputs: Vec::new(),
                clipboard: None,
            })
        })
    }
    fn received(
        &self,
        id: &str,
        wire: WirePoll,
        after: u32,
        frame_after: u32,
    ) -> Result<RemotePoll> {
        self.with(id, |context| {
            if binding(&wire.session) != binding(&context.session) {
                return Err(ClientError::InvalidResponse);
            }
            context.session = wire.session;
            context.session.verification = Some(context.cipher()?.code().into());
            let mut result = RemotePoll {
                cursor: None,
                session: context.session.clone(),
                frame: None,
                frames: Vec::new(),
                inputs: Vec::new(),
                clipboard: None,
            };
            if result.session.phase == RemotePhase::Ended {
                return Ok(result);
            }
            if let Some(keepalive) = wire.keepalive {
                context.observe(&keepalive)?;
            }
            if result.session.phase == RemotePhase::Active
                && context.peer_seen.elapsed() > Duration::from_millis(5000)
            {
                return Err(ClientError::InvalidResponse);
            }
            if result.session.phase != RemotePhase::Active {
                return Ok(result);
            }
            result.cursor.clone_from(&context.peer_cursor);
            if wire.frames.len() > MAX_VIDEO_FRAMES
                || wire.inputs.len() > MAX_BATCH
                || (context.host && !wire.frames.is_empty())
                || (!context.host && (!wire.inputs.is_empty() || wire.clipboard.is_some()))
            {
                return Err(ClientError::InvalidResponse);
            }
            for frame in wire.frames {
                if frame.sequence <= frame_after {
                    return Err(ClientError::InvalidResponse);
                }
                let decoded: RemoteFrame =
                    context.open(&frame.packet, Channel::Frame, frame.sequence)?;
                if decoded.sequence != frame.sequence || !decoded.valid() {
                    return Err(ClientError::InvalidResponse);
                }
                validate_frame(&decoded)?;
                result.frames.push(decoded);
            }
            for event in wire.inputs {
                let (decoded, epoch): (SequencedInput, String) =
                    context.open(&event.packet, Channel::Input, event.sequence)?;
                if decoded.sequence != event.sequence
                    || event.sequence <= after
                    || !decoded.input.valid()
                    || !context.fresh_epoch(&epoch)
                {
                    return Err(ClientError::InvalidResponse);
                }
                result.inputs.push(decoded);
            }
            if let Some(task) = wire.clipboard {
                let (request, deadline, epoch, barrier): (
                    ClipboardRequest,
                    Timestamp,
                    String,
                    u32,
                ) = context.open(&task.request.packet, Channel::ClipboardRequest, 0)?;
                if !request.valid() || request.id != task.request.id {
                    return Err(ClientError::InvalidResponse);
                }
                context.clipboard = Some((
                    request.id.clone(),
                    matches!(request.operation, ClipboardOperation::Read),
                ));
                let expires_at = if context.fresh_epoch(&epoch) {
                    deadline.min(task.request.expires_at)
                } else {
                    Timestamp::from_millis(1).ok_or(ClientError::InvalidResponse)?
                };
                result.clipboard = Some(ClipboardTask {
                    request,
                    expires_at,
                    after_input: task.after_input.max(barrier),
                });
            }
            if result.frames.iter().all(|frame| frame.video.is_none()) {
                result.frame = result.frames.pop();
                result.frames.clear();
            }
            Ok(result)
        })
    }
}
impl Client {
    pub fn remote_verification(&self, id: &str) -> Option<String> {
        self.remote_security
            .with(id, |context| Ok(context.cipher()?.code().to_owned()))
            .ok()
    }
    pub(crate) async fn secure_request(
        &self,
        host_id: &str,
        options: &RequestOptions<'_>,
    ) -> Result<RemoteSession> {
        let known = self
            .remote_security
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .find(|context| !context.host && context.session.host_id == host_id)
            .map(|context| context.session.id.clone());
        if let Some(id) = known {
            if let Some(session) = self
                .remote_sessions(options)
                .await?
                .sessions
                .into_iter()
                .find(|session| session.id == id)
            {
                return Ok(session);
            }
            self.remote_security.remove(&id);
        }
        let (initiator, offer) =
            Initiator::start(host_id).map_err(|_| ClientError::InvalidRequest)?;
        let input = WireCreate {
            host_id: host_id.into(),
            version: SECURE_VERSION,
            offer: STANDARD.encode(offer),
        };
        let mut session: RemoteSession = self
            .remote_json(
                Method::POST,
                "/v1/remote/sessions",
                vec![],
                Some(payload(&input)?),
                options,
            )
            .await?;
        session.verification = None;
        if session.host_id != host_id || session.phase != RemotePhase::Pending {
            return Err(ClientError::InvalidResponse);
        }
        let mut contexts = self
            .remote_security
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if contexts.len() >= 32 {
            return Err(ClientError::InvalidRequest);
        }
        contexts.insert(
            session.id.clone(),
            Context::new(session.clone(), Stage::Initiator(initiator), false)?,
        );
        Ok(session)
    }
    pub async fn remote_prepare_host(
        &self,
        session: &RemoteSession,
        options: &RequestOptions<'_>,
    ) -> Result<()> {
        let handshake: Handshake = self
            .remote_json(
                Method::GET,
                &path(&session.id, "handshake")?,
                vec![],
                None,
                options,
            )
            .await?;
        let response = {
            let mut contexts = self
                .remote_security
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !contexts.contains_key(&session.id) {
                if contexts.len() >= 32 {
                    return Err(ClientError::InvalidRequest);
                }
                let offer = STANDARD
                    .decode(&handshake.offer)
                    .map_err(|_| ClientError::InvalidResponse)?;
                let (responder, response) = Responder::start(&binding(session), &offer)
                    .map_err(|_| ClientError::InvalidResponse)?;
                contexts.insert(
                    session.id.clone(),
                    Context::new(
                        session.clone(),
                        Stage::Responder(responder, STANDARD.encode(response)),
                        true,
                    )?,
                );
            }
            let context = contexts
                .get_mut(&session.id)
                .ok_or(ClientError::InvalidRequest)?;
            if let Some(Stage::Responder(_, response)) = &context.stage {
                Some(response.clone())
            } else {
                None
            }
        };
        if let Some(response) = response {
            self.remote_call(
                Method::POST,
                &path(&session.id, "handshake")?,
                vec![],
                Some(payload(&HandshakeStep {
                    step: 2,
                    data: response,
                })?),
                options,
            )
            .await?;
        }
        if let Some(finish) = handshake.finish {
            self.remote_security.with(&session.id, |context| {
                if matches!(context.stage, Some(Stage::Responder(_, _)))
                    && let Some(Stage::Responder(responder, _)) = context.stage.take()
                {
                    let packet = STANDARD
                        .decode(finish)
                        .map_err(|_| ClientError::InvalidResponse)?;
                    context.stage = Some(Stage::Ready(
                        responder
                            .finish(binding(session), &packet)
                            .map_err(|_| ClientError::InvalidResponse)?,
                        None,
                    ));
                    context.issued = Instant::now();
                    context.peer_seen = Instant::now();
                }
                Ok(())
            })?;
        }
        if self.remote_security.ready(&session.id) {
            let _ = self.secure_poll(&session.id, 0, 0, options).await?;
        }
        Ok(())
    }
    async fn progress_controller(&self, id: &str, options: &RequestOptions<'_>) -> Result<()> {
        let pending = self.remote_security.with(id, |context| {
            Ok(matches!(context.stage, Some(Stage::Initiator(_))))
        })?;
        if pending {
            let handshake: Handshake = self
                .remote_json(Method::GET, &path(id, "handshake")?, vec![], None, options)
                .await?;
            self.remote_security.with(id, |context| {
                context.session = handshake.session;
                if let Some(response) = handshake.response
                    && let Some(Stage::Initiator(initiator)) = context.stage.take()
                {
                    let bytes = STANDARD
                        .decode(response)
                        .map_err(|_| ClientError::InvalidResponse)?;
                    let (cipher, finish) = initiator
                        .finish(binding(&context.session), &bytes)
                        .map_err(|_| ClientError::InvalidResponse)?;
                    context.stage = Some(Stage::Ready(cipher, Some(STANDARD.encode(finish))));
                    context.issued = Instant::now();
                    context.peer_seen = Instant::now();
                }
                Ok(())
            })?;
        }
        let finish = self.remote_security.with(id, |context| {
            Ok(match &context.stage {
                Some(Stage::Ready(_, finish)) => finish.clone(),
                _ => None,
            })
        })?;
        if let Some(finish) = finish {
            self.remote_call(
                Method::POST,
                &path(id, "handshake")?,
                vec![],
                Some(payload(&HandshakeStep {
                    step: 3,
                    data: finish,
                })?),
                options,
            )
            .await?;
            self.remote_security.with(id, |context| {
                if let Some(Stage::Ready(_, finish)) = &mut context.stage {
                    *finish = None;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
    pub(crate) async fn secure_poll(
        &self,
        id: &str,
        after: u32,
        frame_after: u32,
        options: &RequestOptions<'_>,
    ) -> Result<RemotePoll> {
        if !self
            .remote_security
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(id)
        {
            let handshake: Handshake = self
                .remote_json(Method::GET, &path(id, "handshake")?, vec![], None, options)
                .await?;
            if handshake.session.phase == RemotePhase::Ended {
                return Ok(RemotePoll {
                    cursor: None,
                    session: handshake.session,
                    frame: None,
                    frames: Vec::new(),
                    inputs: Vec::new(),
                    clipboard: None,
                });
            }
            return Err(ClientError::InvalidRequest);
        }
        self.progress_controller(id, options).await?;
        if !self.remote_security.ready(id) {
            return self.remote_security.pending(id);
        }
        let input = self.remote_security.poll(id, after, frame_after)?;
        let wire: WirePoll = self
            .remote_json(
                Method::POST,
                &path(id, "poll")?,
                vec![],
                Some(payload(&input)?),
                options,
            )
            .await?;
        self.remote_security.received(id, wire, after, frame_after)
    }
}

#[cfg(test)]
impl Sessions {
    pub(crate) fn test_pair(session: &RemoteSession) -> (Self, Self) {
        let (initiator, offer) = Initiator::start(&session.host_id).unwrap();
        let (responder, response) = Responder::start(&binding(session), &offer).unwrap();
        let (controller, finish) = initiator.finish(binding(session), &response).unwrap();
        let host = responder.finish(binding(session), &finish).unwrap();
        let mut controller =
            Context::new(session.clone(), Stage::Ready(controller, None), false).unwrap();
        let mut host = Context::new(session.clone(), Stage::Ready(host, None), true).unwrap();
        controller.answer = Some(host.challenge.clone());
        host.answer = Some(controller.challenge.clone());
        let a = Self::default();
        let b = Self::default();
        a.0.lock().unwrap().insert(session.id.clone(), host);
        b.0.lock().unwrap().insert(session.id.clone(), controller);
        (a, b)
    }
    pub(crate) fn test_observe(&self, id: &str, packet: &Sealed) {
        self.with(id, |context| context.observe(packet)).unwrap();
    }
    pub(crate) fn test_heartbeat(&self, id: &str) -> Sealed {
        self.with(id, Context::heartbeat).unwrap()
    }
}
