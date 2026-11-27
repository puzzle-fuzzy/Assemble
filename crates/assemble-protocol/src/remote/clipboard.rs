//! 剪贴板只接受用户发起的纯文本操作；请求标识和期限阻止迟到响应串入下一次操作。
use crate::primitives::{Timestamp, is_valid_id};
use serde::{Deserialize, Serialize};

pub const MAX_CLIPBOARD_BYTES: usize = 48 * 1024;
pub const CLIPBOARD_TIMEOUT_MS: i64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClipboardDirection {
    ToHost,
    FromHost,
}

pub fn valid_clipboard(text: &str) -> bool {
    text.len() <= MAX_CLIPBOARD_BYTES && !text.contains('\0')
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ClipboardOperation {
    Read,
    Write { text: String, paste: bool },
}
impl ClipboardOperation {
    pub fn valid(&self) -> bool {
        match self {
            Self::Read => true,
            Self::Write { text, .. } => valid_clipboard(text),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClipboardRequest {
    pub id: String,
    pub operation: ClipboardOperation,
}
impl ClipboardRequest {
    pub fn valid(&self) -> bool {
        is_valid_id(&self.id) && self.operation.valid()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClipboardTask {
    pub request: ClipboardRequest,
    pub expires_at: Timestamp,
    pub after_input: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClipboardStatus {
    Pending,
    Completed,
    Unavailable,
    TooLarge,
    Expired,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClipboardResult {
    pub id: String,
    pub status: ClipboardStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}
impl ClipboardResult {
    pub fn valid(&self) -> bool {
        is_valid_id(&self.id)
            && self.text.as_ref().is_none_or(|text| {
                self.status == ClipboardStatus::Completed && valid_clipboard(text)
            })
    }
}
