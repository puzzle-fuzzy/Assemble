//! 私有托管磁力协议；只有元数据和摘要，不返回源地址、节点或本机路径。
use crate::downloads::{DownloadFile, DownloadPhase};
use crate::primitives::Timestamp;
use serde::{Deserialize, Serialize};

pub const CHUNK_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_REPLY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewInput {
    pub request_id: String,
    pub magnet: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PreviewPhase {
    Resolving,
    Ready,
    Failed,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub id: String,
    pub account_id: String,
    pub phase: PreviewPhase,
    /// 只交给 Rust 校验，不进入界面状态。
    pub metainfo: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Start {
    pub request_id: String,
    pub info_hash: String,
    pub metainfo: String,
    pub file_ids: Vec<u32>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Resume,
    Pause,
    Cancel,
    Release,
    Heartbeat,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Control {
    pub action: Action,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub account_id: String,
    pub info_hash: String,
    pub file_ids: Vec<u32>,
    pub phase: DownloadPhase,
    pub received_bytes: u64,
    pub total_bytes: u64,
    pub bytes_per_second: u64,
    pub error_code: Option<String>,
    pub expires_at: Timestamp,
    pub released: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    #[serde(flatten)]
    pub file: DownloadFile,
    pub sha256: String,
    pub chunks: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub info_hash: String,
    pub chunk_bytes: u64,
    pub files: Vec<File>,
}
