//! 客户端错误的几种来源。
//!
//! 与 Electron 版的错误码一一对应。调用方靠它们区分"服务端拒绝了我的请求"、
//! "服务端的回答看不懂"和"根本没连上"，三者的重试与提示各不相同。

/// 一次请求失败的原因。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    /// 服务端返回了非成功状态。`remote_code` 是它给出的稳定错误码（如果有）。
    #[error("Jarvis request failed ({status})")]
    Http {
        status: u16,
        remote_code: Option<String>,
    },
    /// 请求还没发出去就发现它不合法。
    #[error("Invalid Jarvis request")]
    InvalidRequest,
    /// 服务端成功应答，但内容与约定的结构不符。
    #[error("Jarvis returned an invalid response")]
    InvalidResponse,
    /// 连接失败、断开或响应读了一半。
    #[error("Unable to reach Jarvis")]
    Network,
    #[error("Jarvis request timed out")]
    Timeout,
    #[error("Jarvis request cancelled")]
    Cancelled,
    /// 本机要上传的文件读不出来。
    #[error("the local source could not be read")]
    LocalSource,
}

impl ClientError {
    /// 服务端状态码，仅在服务端确实应答时存在。
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Http { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// 服务端给出的稳定错误码。
    pub fn remote_code(&self) -> Option<&str> {
        match self {
            Self::Http { remote_code, .. } => remote_code.as_deref(),
            _ => None,
        }
    }

    /// 服务端明确回答"这条命令的历史已不存在"。普通的 404 不算。
    pub fn is_command_not_found(&self) -> bool {
        self.status() == Some(404)
            && self.remote_code() == Some(assemble_protocol::commands::COMMAND_NOT_FOUND)
    }

    /// 会话已失效：需要重新登录，而不是重试。
    pub fn is_unauthorized(&self) -> bool {
        self.status() == Some(401)
    }

    /// 值得原样重试的失败：请求没有到达服务端，或者服务端暂时不可用。
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Network | Self::Timeout) || matches!(self.status(), Some(502..=504))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_specific_not_found_counts_as_history_gone() {
        let gone = ClientError::Http {
            status: 404,
            remote_code: Some("COMMAND_NOT_FOUND".into()),
        };
        assert!(gone.is_command_not_found());
        let plain = ClientError::Http {
            status: 404,
            remote_code: Some("NOT_FOUND".into()),
        };
        assert!(!plain.is_command_not_found());
        let no_code = ClientError::Http {
            status: 404,
            remote_code: None,
        };
        assert!(!no_code.is_command_not_found());
        assert!(!ClientError::Network.is_command_not_found());
    }

    #[test]
    fn failures_are_classified_for_retry_and_relogin() {
        assert!(ClientError::Network.is_retryable());
        assert!(ClientError::Timeout.is_retryable());
        assert!(
            ClientError::Http {
                status: 503,
                remote_code: None
            }
            .is_retryable()
        );
        assert!(!ClientError::Cancelled.is_retryable());
        assert!(!ClientError::InvalidResponse.is_retryable());

        let expired = ClientError::Http {
            status: 401,
            remote_code: Some("UNAUTHORIZED".into()),
        };
        assert!(expired.is_unauthorized());
        assert!(!expired.is_retryable());
        assert_eq!(expired.status(), Some(401));
        assert_eq!(expired.remote_code(), Some("UNAUTHORIZED"));
        assert_eq!(ClientError::Timeout.status(), None);
        assert_eq!(ClientError::Timeout.remote_code(), None);
    }

    #[test]
    fn display_never_includes_remote_details() {
        let error = ClientError::Http {
            status: 409,
            remote_code: Some("VERSION_CONFLICT".into()),
        };
        assert_eq!(error.to_string(), "Jarvis request failed (409)");
    }
}
