//! 本机网络下载：受控来源、任务生命周期与已校验文件发布，独立于窗口和正式服务。
#![cfg_attr(test, allow(clippy::pedantic))]

pub mod assistant;
pub mod browser;
mod douyin;
mod hls;
mod magnet;
mod manager;
mod media;
mod network;
mod space;
#[cfg(test)]
mod tests;

pub use manager::DownloadManager;
pub use media::MediaTools;

use assemble_protocol::primitives::to_hex;

/// 错误不携带 URL、Cookie、路径或进程输出，可安全转换为具名命令的稳定错误码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DownloadError {
    #[error("invalid public source")]
    InvalidSource,
    #[error("source is unavailable")]
    SourceUnavailable,
    #[error("download network failed")]
    Network,
    #[error("unsupported playlist or media")]
    Unsupported,
    #[error("download exceeded a resource limit")]
    Limit,
    #[error("downloaded content is incomplete or invalid")]
    InvalidMedia,
    #[error("local download storage failed")]
    Storage,
    #[error("media tools are unavailable")]
    ToolsUnavailable,
    #[error("local media processing failed")]
    Processing,
    #[error("operation cancelled")]
    Cancelled,
    #[error("download reference expired")]
    Expired,
    #[error("download task is busy")]
    Busy,
    #[error("the disk of the download folder does not have enough free space")]
    NoSpace,
    #[error("assistant downloads are disabled")]
    AssistantDisabled,
    #[error("download credentials cannot be stored securely")]
    CredentialsUnavailable,
    #[error("torrent metadata is unavailable")]
    MetadataUnavailable,
    #[error("the explicitly requested direct network is unavailable")]
    NetworkRouteUnavailable,
}

impl DownloadError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidSource => "DOWNLOAD_INVALID_SOURCE",
            Self::SourceUnavailable => "DOWNLOAD_SOURCE_UNAVAILABLE",
            Self::Network => "DOWNLOAD_NETWORK",
            Self::Unsupported => "DOWNLOAD_UNSUPPORTED",
            Self::Limit => "DOWNLOAD_LIMIT",
            Self::InvalidMedia => "DOWNLOAD_INVALID_MEDIA",
            Self::Storage => "DOWNLOAD_STORAGE",
            Self::ToolsUnavailable => "DOWNLOAD_TOOLS_UNAVAILABLE",
            Self::Processing => "DOWNLOAD_PROCESSING",
            Self::Cancelled => "DOWNLOAD_CANCELLED",
            Self::Expired => "DOWNLOAD_EXPIRED",
            Self::Busy => "DOWNLOAD_BUSY",
            Self::NoSpace => "DOWNLOAD_NO_SPACE",
            Self::CredentialsUnavailable => "DOWNLOAD_CREDENTIALS_UNAVAILABLE",
            Self::MetadataUnavailable => "DOWNLOAD_METADATA_UNAVAILABLE",
            Self::NetworkRouteUnavailable => "DOWNLOAD_ROUTE_UNAVAILABLE",
            Self::AssistantDisabled => "DOWNLOAD_ASSISTANT_DISABLED",
        }
    }
}

pub(crate) type Result<T> = std::result::Result<T, DownloadError>;

pub(crate) fn random_id() -> Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| DownloadError::Storage)?;
    Ok(to_hex(&bytes))
}

pub(crate) const MAX_BYTES: u64 = 10 * 1024 * 1024 * 1024;
pub(crate) const MAX_METADATA: u64 = 4 * 1024 * 1024;
/// 下载目录所在的磁盘至少要剩这么多，才开始写。留给系统和别人正在用的程序；
/// 点播清单合并时还要再写一份成品，所以不是"刚好放得下"就够。
pub(crate) const MIN_FREE_BYTES: u64 = 200 * 1024 * 1024;
