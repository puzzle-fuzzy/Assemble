//! 传输原语：把一段字节流安全地落到本机，或从本机读出并校验。
//!
//! 三条不变量沿用自 Electron 版：
//!
//! - 只复制，不移动，绝不覆盖已有的不同文件；
//! - 内容先写进同目录的 `.part` 暂存文件，校验通过后才发布；
//! - 摘要边传边算，任何时刻都不把整个文件读进内存。
//!
//! 这里不判断"这个目录能不能访问"。授权与路径边界由 `assemble-device`
//! 负责；调用方必须在暂存之前与发布之前各校验一次。

// 测试代码以可读性优先，不套用库代码的严格检查。
#![cfg_attr(test, allow(clippy::pedantic))]

pub mod download;
pub mod hash;
pub mod source;
pub mod staged;

/// 一次传输失败的原因。`code` 是回执里上报的稳定错误码。
#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error("use an HTTP(S) download URL")]
    InvalidUrl,
    #[error("a magnet downloader has not been configured")]
    MagnetNotConfigured,
    #[error("the source is not a regular file")]
    NotRegularFile,
    #[error("the source file changed while it was being read")]
    SourceChanged,
    #[error("download response has no body")]
    EmptyResponse,
    #[error("downloaded size does not match the response")]
    IncompleteDownload,
    #[error("transfer integrity check failed")]
    HashMismatch,
    #[error("a different file already exists; choose another name")]
    FileExists,
    #[error("the transfer exceeds the allowed size")]
    TooLarge,
    #[error("operation cancelled")]
    Cancelled,
    #[error("transfer I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("a unique staging name could not be generated")]
    RandomUnavailable,
}

impl TransferError {
    /// 本机使用的稳定错误码，命名与 Electron 版一致。
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidUrl => "INVALID_URL",
            Self::MagnetNotConfigured => "MAGNET_NOT_CONFIGURED",
            Self::SourceChanged => "SOURCE_CHANGED",
            Self::EmptyResponse => "EMPTY_RESPONSE",
            Self::IncompleteDownload => "INCOMPLETE_DOWNLOAD",
            Self::HashMismatch => "HASH_MISMATCH",
            Self::FileExists => "FILE_EXISTS",
            // 不是普通文件与超过上限共用一个错误码：两者都是"这个文件不能上云"。
            Self::NotRegularFile | Self::TooLarge => "FILE_TOO_LARGE",
            Self::Cancelled => "CANCELLED",
            Self::Io(_) | Self::RandomUnavailable => "DEVICE_ERROR",
        }
    }

    /// 可以写进回执、日志或 RPC 结果的说明：不含路径与系统诊断。
    pub fn public_message(&self) -> &'static str {
        match self {
            Self::InvalidUrl => "Use an HTTP(S) download URL",
            Self::MagnetNotConfigured => "A magnet downloader has not been configured",
            Self::NotRegularFile => {
                "Only regular files smaller than 100,000,000 bytes can be uploaded"
            }
            Self::SourceChanged => "Source file changed since it was requested; resolve it again",
            Self::EmptyResponse => "Download response has no body",
            Self::IncompleteDownload => "Downloaded size does not match the response",
            Self::HashMismatch => "Downloaded file integrity check failed",
            Self::FileExists => "A different file already exists; choose another name",
            Self::TooLarge => "The transfer exceeds the allowed size",
            Self::Cancelled => "Operation cancelled",
            Self::Io(_) | Self::RandomUnavailable => {
                "Operation failed; check connection, permissions and cloud status"
            }
        }
    }
}

/// 读写时每次处理的字节数。
pub(crate) const CHUNK_BYTES: usize = 64 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_messages_never_carry_paths_or_system_detail() {
        let error = TransferError::Io(std::io::Error::other(
            "/Users/example/secret.txt: permission denied (EACCES)",
        ));
        assert_eq!(error.code(), "DEVICE_ERROR");
        let message = error.public_message();
        assert!(!message.contains("secret.txt"));
        assert!(!message.contains("EACCES"));
        // 开发诊断里保留原始原因，对外说明里没有。
        assert!(error.to_string().contains("EACCES"));
    }

    #[test]
    fn an_unreadable_file_reports_the_same_code_as_an_oversized_one() {
        assert_eq!(TransferError::NotRegularFile.code(), "FILE_TOO_LARGE");
        assert_eq!(TransferError::TooLarge.code(), "FILE_TOO_LARGE");
        assert_eq!(TransferError::FileExists.code(), "FILE_EXISTS");
        assert_eq!(TransferError::SourceChanged.code(), "SOURCE_CHANGED");
        assert_eq!(
            TransferError::FileExists.public_message(),
            "A different file already exists; choose another name"
        );
    }
}
