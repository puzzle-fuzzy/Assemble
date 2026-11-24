//! 非 2xx 响应的正文与稳定错误码。
//!
//! 正文是 `{ "error": { "code", "message" } }`。`message` 是给开发者看的英文说明，
//! 可以随时改；`code` 是稳定不变的，桌面端据此决定提示与重试。

use serde::{Deserialize, Serialize};

/// 服务端会给出的稳定错误码。
pub mod codes {
    /// 没带令牌，或令牌已失效：需要重新登录。
    pub const UNAUTHORIZED: &str = "UNAUTHORIZED";
    /// 已登录，但这条资源不属于这个账户，或不能由这台设备操作。
    pub const FORBIDDEN: &str = "FORBIDDEN";
    pub const NOT_FOUND: &str = "NOT_FOUND";
    /// 请求本身不合法：缺字段、格式不对、超出取值范围。
    pub const INVALID_REQUEST: &str = "INVALID_REQUEST";
    /// 目标设备此刻不在线。上传途中它下线了也是这个码。
    pub const DEVICE_OFFLINE: &str = "DEVICE_OFFLINE";
    /// 文件超过单文件上限。
    pub const FILE_TOO_LARGE: &str = "FILE_TOO_LARGE";
    /// 内容与声明的摘要不一致，或长度与声明的不符。
    pub const HASH_MISMATCH: &str = "HASH_MISMATCH";
    /// 云盘空间不够。
    pub const QUOTA_EXCEEDED: &str = "QUOTA_EXCEEDED";
    /// 一次发送已不在可以执行这个操作的状态（比如已经送达又来拒绝）。
    pub const SEND_STATE_CONFLICT: &str = "SEND_STATE_CONFLICT";
    /// 用户名或密码不对。两个原因用同一个码，不透露哪一个错了。
    pub const LOGIN_FAILED: &str = "LOGIN_FAILED";
    /// 连续失败太多次，暂时拒绝。
    pub const TOO_MANY_ATTEMPTS: &str = "TOO_MANY_ATTEMPTS";
    /// 授权码不存在、已用过或已过期，需要重新开始登录。
    pub const INVALID_AUTH_CODE: &str = "INVALID_AUTH_CODE";
    /// 修改时带的版本号与服务端的不一致：这条记录在别处被改过。刷新之后重来。
    pub const VERSION_CONFLICT: &str = "VERSION_CONFLICT";
    /// 这条记录现在的状态不允许这个操作（比如已经到点的提醒不能再改时间）。
    pub const INVALID_STATE: &str = "INVALID_STATE";
    /// 网页表单的防伪校验没通过。
    pub const CSRF_REJECTED: &str = "CSRF_REJECTED";
    pub const UNAVAILABLE: &str = "UNAVAILABLE";
    pub const INTERNAL: &str = "INTERNAL";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn error_body_exposes_code_and_message() {
        let body: ApiErrorBody = serde_json::from_value(json!({
            "error": { "code": "DEVICE_OFFLINE", "message": "target device is offline" }
        }))
        .unwrap();
        assert_eq!(body.error.code, codes::DEVICE_OFFLINE);
        // 未知字段不影响读取。
        let extended: ApiErrorBody = serde_json::from_value(json!({
            "error": { "code": "X", "message": "m", "requestId": "r1" }
        }))
        .unwrap();
        assert_eq!(extended.error.code, "X");
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            json!({ "error": { "code": "DEVICE_OFFLINE", "message": "target device is offline" } })
        );
    }
}
