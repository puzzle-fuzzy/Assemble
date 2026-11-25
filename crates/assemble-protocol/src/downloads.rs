//! 本机网络下载契约。来源只单向交给 Rust；展示状态不包含完整 URL、密钥或磁盘路径。

use serde::{Deserialize, Serialize};

use crate::primitives::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadKind {
    Hls,
    Douyin,
    Video,
}

/// 后续浏览器插件可复用此输入；不接受 Cookie、任意请求头、文件路径或进程参数。
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadInput {
    pub kind: DownloadKind,
    pub url: String,
    #[serde(default)]
    pub page_url: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadVariant {
    pub id: String,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub bandwidth: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadPreview {
    pub id: String,
    pub kind: DownloadKind,
    pub title: String,
    pub source_host: String,
    pub duration_seconds: Option<f64>,
    pub variants: Vec<DownloadVariant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadPhase {
    Queued,
    Downloading,
    Muxing,
    Verifying,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl DownloadPhase {
    pub fn active(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Downloading | Self::Muxing | Self::Verifying
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTask {
    pub id: String,
    pub kind: DownloadKind,
    pub title: String,
    pub file_name: Option<String>,
    pub source_host: String,
    pub phase: DownloadPhase,
    pub received_bytes: u64,
    pub total_bytes: Option<u64>,
    pub completed_segments: u64,
    pub total_segments: Option<u64>,
    pub bytes_per_second: u64,
    pub media_seconds: Option<f64>,
    pub duration_seconds: Option<f64>,
    pub error_code: Option<String>,
    pub retryable: bool,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadsView {
    pub browser_task_id: Option<String>,
    pub folder_name: String,
    pub tools_available: bool,
    pub tasks: Vec<DownloadTask>,
}
