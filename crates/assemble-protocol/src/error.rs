//! 服务端的统一错误响应。

use serde::{Deserialize, Serialize};

/// 字段级校验问题：只公开路径与稳定的错误码，不回显输入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<ValidationIssue>>,
}

/// 非 2xx 响应的正文：`{ error: { code, message, requestId, fields? } }`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: ErrorDetail,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn error_body_exposes_code_message_and_optional_details() {
        let body: ApiErrorBody = serde_json::from_value(json!({
            "error": {
                "code": "VERSION_CONFLICT",
                "message": "版本冲突",
                "requestId": "req-1",
                "fields": [{ "path": "data.title", "code": "too_small" }]
            }
        }))
        .unwrap();
        assert_eq!(body.error.code, "VERSION_CONFLICT");
        let Some(fields) = body.error.fields else {
            panic!("expected field issues");
        };
        assert_eq!(fields[0].path.as_deref(), Some("data.title"));
        assert!(fields[0].message.is_none());
    }

    #[test]
    fn error_body_tolerates_missing_optional_parts() {
        let body: ApiErrorBody = serde_json::from_value(
            json!({ "error": { "code": "NOT_FOUND", "message": "missing" } }),
        )
        .unwrap();
        assert!(body.error.request_id.is_none());
        assert!(body.error.fields.is_none());
        let value: ApiErrorBody =
            serde_json::from_value(json!({ "error": { "code": "X", "message": "m" }, "extra": 1 }))
                .unwrap();
        assert_eq!(value.error.code, "X");
    }
}
