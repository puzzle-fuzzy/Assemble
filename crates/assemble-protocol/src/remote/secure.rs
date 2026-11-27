//! 中转只接触有界密文与路由元数据；业务类型在两端 Rust 内解密并重新校验。
use super::RemoteSession;
use crate::primitives::Timestamp;
use serde::{Deserialize, Serialize};
pub const SECURE_VERSION: u8 = 2;
pub const MAX_PLAIN_BYTES: usize = super::MAX_FRAME_BASE64 + 64 * 1024;
pub const MAX_SEALED_BASE64: usize = (MAX_PLAIN_BYTES + 16 * 16).div_ceil(3) * 4 + 64;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Sealed {
    pub nonce: u64,
    pub chunks: Vec<String>,
}
impl Sealed {
    pub fn valid(&self) -> bool {
        !self.chunks.is_empty()
            && self.chunks.len() <= 16
            && self.nonce.checked_add(self.chunks.len() as u64).is_some()
            && self
                .chunks
                .iter()
                .all(|chunk| !chunk.is_empty() && chunk.len() <= 82_000)
            && self.chunks.iter().map(String::len).sum::<usize>() <= MAX_SEALED_BASE64
    }
    pub fn bytes(&self) -> usize {
        self.chunks.iter().map(String::len).sum()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Channel {
    Frame,
    Input,
    ClipboardRequest,
    ClipboardResult,
    AudioState,
    AudioPacket,
    Heartbeat,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub session_id: String,
    pub controller_id: String,
    pub host_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireCreate {
    pub host_id: String,
    pub version: u8,
    pub offer: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Handshake {
    pub session: RemoteSession,
    pub offer: String,
    pub response: Option<String>,
    pub finish: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HandshakeStep {
    pub step: u8,
    pub data: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireFrame {
    pub sequence: u32,
    pub packet: Sealed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireInput {
    pub sequence: u32,
    pub packet: Sealed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireBatch {
    pub events: Vec<WireInput>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WirePollRequest {
    pub after: u32,
    pub frame_after: u32,
    pub keepalive: Sealed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WirePoll {
    pub session: RemoteSession,
    pub frames: Vec<WireFrame>,
    pub inputs: Vec<WireInput>,
    pub clipboard: Option<WireClipboardTask>,
    pub keepalive: Option<Sealed>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireClipboardRequest {
    pub id: String,
    pub expires_at: Timestamp,
    pub packet: Sealed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireClipboardTask {
    pub request: WireClipboardRequest,
    pub after_input: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireClipboardResult {
    pub id: String,
    pub expired: bool,
    pub packet: Option<Sealed>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireAudioPacket {
    pub sequence: u32,
    pub packet: Sealed,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireAudio {
    pub state: Option<Sealed>,
    pub packets: Vec<WireAudioPacket>,
}
