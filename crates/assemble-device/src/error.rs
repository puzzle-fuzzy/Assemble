//! 设备侧的错误，以及把它们安全地交给外界的规则。
//!
//! 回执、网关响应和日志里只允许出现这里定义的稳定错误码与固定说明。
//! 路径、系统诊断和网络细节留在开发日志里，不出本机。

use std::borrow::Cow;

use assemble_protocol::gateway::GatewayError;
use assemble_store::{json::StoreError, secret::SecretError};
use assemble_transfer::TransferError;

/// 兜底说明：足以让用户知道该检查什么，又不泄露任何本机信息。
pub const GENERIC_FAILURE: &str =
    "Operation failed; check connection, permissions and cloud status";

/// 本机效果已完成、但云端还没有确认时，任务上显示的说明。
pub const AWAITING_ACKNOWLEDGEMENT: &str = "Awaiting cloud acknowledgement";

/// 设备侧的一次失败：稳定的错误码加一句可以对外说明的话。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct DeviceError {
    pub code: &'static str,
    pub message: Cow<'static, str>,
}

impl DeviceError {
    const fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message: Cow::Borrowed(message),
        }
    }

    pub const ROOT_NOT_ABSOLUTE: Self = Self::new("INVALID_ROOT", "Choose an absolute directory");
    pub const ROOT_IS_DISK: Self = Self::new("INVALID_ROOT", "A whole disk cannot be shared");
    pub const ROOT_IS_LINK: Self = Self::new("INVALID_ROOT", "Choose a real directory, not a link");
    pub const ROOT_IS_LINKED: Self =
        Self::new("INVALID_ROOT", "Linked directories cannot be shared");

    pub const PATH_NOT_RELATIVE: Self = Self::new("INVALID_PATH", "Use a safe relative file path");
    pub const PATH_PARENT_NOT_DIRECTORY: Self =
        Self::new("INVALID_PATH", "A parent path is not a directory");
    pub const PATH_EXCLUDED: Self = Self::new(
        "EXCLUDED_PATH",
        "This credential or generated path is excluded",
    );
    pub const LINK_IS_SYMLINK: Self = Self::new("LINK_ESCAPE", "Symbolic links are not shared");
    pub const LINK_LEAVES_ROOT: Self = Self::new("LINK_ESCAPE", "Path leaves the shared directory");

    pub const NOT_FOUND: Self = Self::new("ENOENT", GENERIC_FAILURE);
    pub const NOT_REGISTERED: Self = Self::new("NOT_REGISTERED", "Connect this device first");
    pub const ROOT_NOT_AUTHORIZED: Self = Self::new(
        "ROOT_NOT_AUTHORIZED",
        "Choose an enabled shared directory on this device",
    );
    pub const IDENTITY_REQUIRED: Self = Self::new(
        "AUTH_IDENTITY_REQUIRED",
        "Confirm the signed-in account before granting a directory",
    );
    pub const PICKER_UNAVAILABLE: Self = Self::new(
        "DIRECTORY_PICKER_UNAVAILABLE",
        "Directory authorization is unavailable on this device",
    );
    pub const FILE_TOO_LARGE: Self = Self::new(
        "FILE_TOO_LARGE",
        "Only regular files smaller than 100,000,000 bytes can be uploaded",
    );
    pub const NOTIFICATIONS_UNAVAILABLE: Self = Self::new(
        "NOTIFICATIONS_UNAVAILABLE",
        "System notification delivery is unavailable",
    );
    pub const SESSION_EXPIRED: Self =
        Self::new("SESSION_EXPIRED", "Sign in again to reconnect this device");
    pub const CANCELLED: Self = Self::new("CANCELLED", "Operation cancelled");
    pub const INVALID_CLOUD_URL: Self = Self::new(
        "INVALID_CLOUD_URL",
        "Use an HTTPS origin, or HTTP on loopback for development",
    );
    pub const CREDENTIAL_STORE_UNAVAILABLE: Self =
        Self::new("UNAVAILABLE", "The system credential store is unavailable");
    /// 兜底：原因不明，或原因不适合对外说明。
    pub const FAILED: Self = Self::new("DEVICE_ERROR", GENERIC_FAILURE);

    /// 调用方漏了必填字段。
    pub fn missing_input(field: &str) -> Self {
        Self {
            code: "INVALID_INPUT",
            message: Cow::Owned(format!("{field} is required")),
        }
    }
}

/// 把一次系统调用失败记进开发日志，再收敛成不泄露细节的错误。
pub fn io_failure(operation: &'static str, error: &std::io::Error) -> DeviceError {
    tracing::debug!(operation, kind = ?error.kind(), "device filesystem operation failed");
    DeviceError::FAILED
}

impl From<TransferError> for DeviceError {
    fn from(error: TransferError) -> Self {
        match error {
            // 系统诊断不出本机。
            TransferError::Io(_) | TransferError::RandomUnavailable => Self::FAILED,
            known => Self {
                code: known.code(),
                message: Cow::Borrowed(known.public_message()),
            },
        }
    }
}

impl From<StoreError> for DeviceError {
    fn from(error: StoreError) -> Self {
        tracing::debug!(%error, "local state could not be used");
        Self::FAILED
    }
}

impl From<SecretError> for DeviceError {
    fn from(error: SecretError) -> Self {
        tracing::debug!(%error, "credential store rejected a request");
        Self::CREDENTIAL_STORE_UNAVAILABLE
    }
}

impl From<DeviceError> for GatewayError {
    fn from(error: DeviceError) -> Self {
        Self {
            code: error.code.to_owned(),
            message: error.message.into_owned(),
        }
    }
}

/// 把错误收敛成网关的错误形状：已知的保留错误码，其余一律兜底。
///
/// 没有列在这里的类型不会被自动放行；要对外返回就得先明确它该归到哪一类。
pub trait PublicError {
    fn public_error(&self) -> GatewayError;
}

impl PublicError for DeviceError {
    fn public_error(&self) -> GatewayError {
        self.clone().into()
    }
}

impl PublicError for TransferError {
    fn public_error(&self) -> GatewayError {
        match self {
            TransferError::Io(_) | TransferError::RandomUnavailable => DeviceError::FAILED.into(),
            known => GatewayError {
                code: known.code().to_owned(),
                message: known.public_message().to_owned(),
            },
        }
    }
}

impl PublicError for GatewayError {
    fn public_error(&self) -> GatewayError {
        self.clone()
    }
}

impl PublicError for StoreError {
    fn public_error(&self) -> GatewayError {
        tracing::debug!(%self, "local state could not be used");
        DeviceError::FAILED.into()
    }
}

impl PublicError for SecretError {
    fn public_error(&self) -> GatewayError {
        DeviceError::CREDENTIAL_STORE_UNAVAILABLE.into()
    }
}

impl PublicError for std::io::Error {
    fn public_error(&self) -> GatewayError {
        DeviceError::FAILED.into()
    }
}

impl PublicError for serde_json::Error {
    fn public_error(&self) -> GatewayError {
        DeviceError::FAILED.into()
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use super::*;

    #[test]
    fn known_errors_keep_their_stable_code_and_message() {
        let error = DeviceError::ROOT_IS_DISK;
        let public = error.public_error();
        assert_eq!(public.code, "INVALID_ROOT");
        assert_eq!(public.message, "A whole disk cannot be shared");
        assert_eq!(error.to_string(), "A whole disk cannot be shared");
        assert_eq!(
            DeviceError::missing_input("rootId").public_error(),
            GatewayError {
                code: "INVALID_INPUT".into(),
                message: "rootId is required".into(),
            }
        );
    }

    #[test]
    fn system_diagnostics_never_leave_the_device() {
        let io = std::io::Error::other("/Users/example/private/.ssh/id_rsa: EACCES");
        let public = io.public_error();
        assert_eq!(public.code, "DEVICE_ERROR");
        assert_eq!(public.message, GENERIC_FAILURE);

        let transfer = TransferError::Io(std::io::Error::other("C:\\Users\\secret: EBUSY"));
        let public = transfer.public_error();
        assert_eq!(public.code, "DEVICE_ERROR");
        assert!(!public.message.contains("secret"));
        assert!(!public.message.contains(':'));
    }

    #[test]
    fn transfer_failures_map_to_their_public_explanation() {
        let error = DeviceError::from(TransferError::FileExists);
        assert_eq!(error.code, "FILE_EXISTS");
        assert_eq!(
            error.public_error().message,
            "A different file already exists; choose another name"
        );
        assert_eq!(
            DeviceError::from(TransferError::SourceChanged).code,
            "SOURCE_CHANGED"
        );
        assert_eq!(
            DeviceError::from(TransferError::Cancelled),
            DeviceError::CANCELLED
        );
    }

    #[test]
    fn storage_and_credential_failures_collapse_to_safe_errors() {
        let store: DeviceError = StoreError::Corrupt {
            path: "/private/state.json".into(),
            source: serde_json::from_str::<u8>("{").unwrap_err(),
        }
        .into();
        assert_eq!(store, DeviceError::FAILED);
        assert_eq!(
            DeviceError::from(SecretError::Denied),
            DeviceError::CREDENTIAL_STORE_UNAVAILABLE
        );
        assert_eq!(
            DeviceError::CREDENTIAL_STORE_UNAVAILABLE.code,
            "UNAVAILABLE"
        );
    }

    #[test]
    fn filesystem_failures_are_logged_then_generalised() {
        let error = io_failure(
            "read_dir",
            &std::io::Error::from(ErrorKind::PermissionDenied),
        );
        assert_eq!(error, DeviceError::FAILED);
        // "不存在"保留它的系统错误码，其余系统调用失败一律兜底。
        assert_eq!(DeviceError::NOT_FOUND.code, "ENOENT");
        assert_eq!(DeviceError::NOT_FOUND.message, GENERIC_FAILURE);
    }

    #[test]
    fn error_codes_are_unique_per_meaning() {
        let all = [
            DeviceError::ROOT_NOT_ABSOLUTE,
            DeviceError::PATH_NOT_RELATIVE,
            DeviceError::PATH_EXCLUDED,
            DeviceError::LINK_IS_SYMLINK,
            DeviceError::NOT_FOUND,
            DeviceError::NOT_REGISTERED,
            DeviceError::ROOT_NOT_AUTHORIZED,
            DeviceError::IDENTITY_REQUIRED,
            DeviceError::PICKER_UNAVAILABLE,
            DeviceError::FILE_TOO_LARGE,
            DeviceError::NOTIFICATIONS_UNAVAILABLE,
            DeviceError::SESSION_EXPIRED,
            DeviceError::CANCELLED,
            DeviceError::INVALID_CLOUD_URL,
            DeviceError::CREDENTIAL_STORE_UNAVAILABLE,
            DeviceError::FAILED,
        ];
        let mut codes: Vec<_> = all.iter().map(|error| error.code).collect();
        let total = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), total);
        for error in all {
            assert!(!error.message.is_empty());
        }
    }
}
