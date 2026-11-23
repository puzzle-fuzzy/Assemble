//! 客户端错误的几种来源。
//!
//! 调用方靠它们区分"服务端拒绝了我的请求"、"服务端的回答看不懂"和
//! "根本没连上"，三者的重试与提示各不相同。显示文字里不含服务端给出的细节。

/// 一次请求失败的原因。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    /// 服务端返回了非成功状态。`remote_code` 是它给出的稳定错误码（如果有）。
    #[error("the server rejected the request ({status})")]
    Http {
        status: u16,
        remote_code: Option<String>,
    },
    /// 请求还没发出去就发现它不合法。
    #[error("invalid request")]
    InvalidRequest,
    /// 服务端成功应答，但内容与约定的结构不符。
    #[error("the server returned an invalid response")]
    InvalidResponse,
    /// 连接失败、断开或响应读了一半。
    #[error("unable to reach the server")]
    Network,
    /// 超过时限，或者传输中途卡住太久。
    #[error("request timed out")]
    Timeout,
    #[error("request cancelled")]
    Cancelled,
    /// 要发的文件超过单文件上限。
    #[error("the file exceeds the size limit")]
    TooLarge,
    /// 本机要上传的文件读不出来，或它在读取途中变了。
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

    /// 服务端明确表示这条凭据不再有效：需要重新登录，而不是重试。
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
        // 403 是"不允许"，不是"没登录"。
        assert!(
            !ClientError::Http {
                status: 403,
                remote_code: None
            }
            .is_unauthorized()
        );
    }

    #[test]
    fn display_never_includes_remote_details() {
        let error = ClientError::Http {
            status: 409,
            remote_code: Some("DEVICE_OFFLINE".into()),
        };
        assert_eq!(error.to_string(), "the server rejected the request (409)");
    }
}
