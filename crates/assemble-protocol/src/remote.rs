//! 远程桌面的具名协议。画面只保留最新一帧，输入按序确认，绝不无界排队。
use crate::primitives::Timestamp;
use serde::{Deserialize, Serialize};

mod clipboard;
pub use clipboard::*;
mod audio;
pub use audio::*;
pub mod secure;

pub const MAX_FRAME_BYTES: usize = 512 * 1024;
pub const MAX_FRAME_BASE64: usize = MAX_FRAME_BYTES.div_ceil(3) * 4;
pub const MAX_INPUTS: usize = 128;
pub const MAX_BATCH: usize = 32;
pub const REQUEST_TTL_MS: i64 = 60_000;
pub const PEER_TIMEOUT_MS: i64 = 5_000;
pub const MAX_VIDEO_FRAMES: usize = 8;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteQuality {
    #[default]
    Auto,
    Smooth,
    Balanced,
    Sharp,
}
pub fn balanced_quality() -> RemoteQuality {
    RemoteQuality::Balanced
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemotePhase {
    Pending,
    Active,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteEndReason {
    Stopped,
    Declined,
    Expired,
    Disconnected,
    DeviceRevoked,
    CaptureFailed,
    InputFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteEnd {
    pub reason: RemoteEndReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSession {
    pub id: String,
    pub controller_id: String,
    pub controller_name: String,
    pub host_id: String,
    pub host_name: String,
    pub phase: RemotePhase,
    pub created_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<RemoteEndReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteView {
    pub sessions: Vec<RemoteSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteRequest {
    pub host_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDisplay {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFrame {
    pub sequence: u32,
    pub display_id: u32,
    pub width: u32,
    pub height: u32,
    pub jpeg: String,
    pub displays: Vec<RemoteDisplay>,
    #[serde(default = "balanced_quality")]
    pub quality: RemoteQuality,
    #[serde(default)]
    pub requested_quality: RemoteQuality,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<RemoteVideo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<RemoteCursor>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteCursor {
    pub x: u16,
    pub y: u16,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CursorUpdate {
    pub display_id: u32,
    pub position: Option<RemoteCursor>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteVideo {
    pub stream: u32,
    pub codec: String,
    pub keyframe: bool,
    pub timestamp: u64,
    pub data: String,
}
impl RemoteVideo {
    pub fn valid(&self) -> bool {
        self.stream > 0
            && self.codec.len() == 11
            && self.codec.starts_with("avc1.")
            && self.codec.as_bytes()[5..].iter().all(u8::is_ascii_hexdigit)
            && self.timestamp <= 9_007_199_254_740_991
            && !self.data.is_empty()
            && self.data.len() <= MAX_FRAME_BASE64
    }
}
impl RemoteFrame {
    pub fn valid(&self) -> bool {
        self.sequence > 0
            && self.quality != RemoteQuality::Auto
            && self.width > 0
            && self.height > 0
            && self.width <= 1920
            && self.height <= 1920
            && self.video.as_ref().map_or_else(
                || !self.jpeg.is_empty() && self.jpeg.len() <= MAX_FRAME_BASE64,
                |video| self.jpeg.is_empty() && video.valid(),
            )
            && !self.displays.is_empty()
            && self.displays.len() <= 16
            && self.displays.iter().any(|d| d.id == self.display_id)
            && self.displays.iter().all(|d| {
                d.width > 0
                    && d.height > 0
                    && d.width <= 32768
                    && d.height <= 32768
                    && d.name.len() <= 256
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RemoteInput {
    Move { x: u16, y: u16 },
    Button { button: RemoteButton, pressed: bool },
    Key { key: String, pressed: bool },
    Scroll { x: i16, y: i16 },
    Text { text: String },
    Display { id: u32 },
    ReleaseAll,
    Quality { quality: RemoteQuality },
    Video { enabled: bool },
    Keyframe,
    Audio { enabled: bool },
}
pub fn valid_key(key: &str) -> bool {
    (key.chars().count() == 1 && key.chars().all(|c| !c.is_control()))
        || matches!(
            key,
            "Enter"
                | "Tab"
                | "Backspace"
                | "Escape"
                | "Delete"
                | "Insert"
                | "Home"
                | "End"
                | "PageUp"
                | "PageDown"
                | "ArrowUp"
                | "ArrowDown"
                | "ArrowLeft"
                | "ArrowRight"
                | "Control"
                | "Shift"
                | "Alt"
                | "Meta"
                | "F1"
                | "F2"
                | "F3"
                | "F4"
                | "F5"
                | "F6"
                | "F7"
                | "F8"
                | "F9"
                | "F10"
                | "F11"
                | "F12"
        )
}
impl RemoteInput {
    pub fn requires_display(&self) -> bool {
        matches!(
            self,
            Self::Move { .. }
                | Self::Button { .. }
                | Self::Key { .. }
                | Self::Scroll { .. }
                | Self::Text { .. }
        )
    }
    pub fn valid(&self) -> bool {
        match self {
            Self::Key { key, .. } => valid_key(key),
            Self::Text { text } => !text.is_empty() && text.len() <= 4096 && !text.contains('\0'),
            Self::Scroll { x, y } => x.unsigned_abs() <= 20 && y.unsigned_abs() <= 20,
            _ => true,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SequencedInput {
    pub sequence: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_id: Option<u32>,
    pub input: RemoteInput,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteBatch {
    pub events: Vec<SequencedInput>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePoll {
    pub session: RemoteSession,
    pub frame: Option<RemoteFrame>,
    pub inputs: Vec<SequencedInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clipboard: Option<ClipboardTask>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<RemoteFrame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<CursorUpdate>,
}
