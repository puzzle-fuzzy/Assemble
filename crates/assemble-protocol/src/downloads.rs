//! 本机网络下载契约。来源只单向交给 Rust；展示状态不包含完整 URL、密钥或磁盘路径。

use serde::{Deserialize, Serialize};

use crate::primitives::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadKind {
    Hls,
    Douyin,
    Video,
    Magnet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadInitiator {
    Assistant,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadFile {
    pub id: u32,
    pub name: String,
    pub size: u64,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<DownloadFile>,
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
    Paused,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initiated_by: Option<DownloadInitiator>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadsView {
    #[serde(default = "assistant_allowed_default")]
    pub assistant_allowed: bool,
    pub browser_task_id: Option<String>,
    pub folder_name: String,
    pub tools_available: bool,
    pub tasks: Vec<DownloadTask>,
}

pub fn assistant_allowed_default() -> bool {
    true
}

impl Default for DownloadsView {
    fn default() -> Self {
        Self {
            assistant_allowed: true,
            browser_task_id: None,
            folder_name: String::new(),
            tools_available: false,
            tasks: Vec::new(),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssistantDownloadStart {
    pub request_id: String,
    pub preview_id: String,
    pub variant_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_ids: Vec<u32>,
}

/// 只投影安全展示字段；来源 URL、系统凭据与落盘路径永远不在这里。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssistantDownloadReply {
    pub version: u8,
    pub ok: bool,
    pub preview: Option<DownloadPreview>,
    pub task: Option<DownloadTask>,
    pub task_id: Option<String>,
    pub error_code: Option<String>,
}

impl AssistantDownloadReply {
    pub fn success() -> Self {
        Self {
            version: 1,
            ok: true,
            preview: None,
            task: None,
            task_id: None,
            error_code: None,
        }
    }

    pub fn failure(code: &str) -> Self {
        Self {
            ok: false,
            error_code: Some(code.to_owned()),
            ..Self::success()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn video_start_preserves_the_v1_request_for_older_desktops() {
        let legacy = serde_json::json!({"requestId":"once","previewId":"preview","variantId":"hd"});
        let start: AssistantDownloadStart = serde_json::from_value(legacy.clone()).unwrap();
        assert!(start.file_ids.is_empty());
        assert_eq!(serde_json::to_value(start).unwrap(), legacy);
    }
}
