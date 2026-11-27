//! 接口错误：统一成带稳定错误码的 JSON。
//!
//! 桌面端只认 `code`；`message` 是给开发看的英文说明，可以随时改。
//! 真正的技术细节（数据库错误等）只进日志，不回给调用方。

use assemble_protocol::error::{ApiErrorBody, ApiErrorDetail, codes};
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// 没带令牌、令牌无效，或这台设备已被撤销。
    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            codes::UNAUTHORIZED,
            "sign in again",
        )
    }

    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, codes::NOT_FOUND, "no such resource")
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, codes::INVALID_REQUEST, message)
    }

    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, codes::INTERNAL, message)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {}: {}",
            self.status.as_u16(),
            self.code,
            self.message
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let retry_after = self.code == "MEDIA_CATALOG_RATE_LIMITED";
        let mut response = (
            self.status,
            Json(ApiErrorBody {
                error: ApiErrorDetail {
                    code: self.code.to_owned(),
                    message: self.message,
                },
            }),
        )
            .into_response();
        if retry_after {
            response.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                axum::http::HeaderValue::from_static("60"),
            );
        }
        response
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        // 数据库的原话里可能带着语句片段，只进日志。
        tracing::error!(%error, "database operation failed");
        Self::internal("a database operation failed")
    }
}

impl From<std::io::Error> for ApiError {
    fn from(error: std::io::Error) -> Self {
        tracing::warn!(%error, "a storage operation failed");
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            codes::UNAVAILABLE,
            "storage is temporarily unavailable",
        )
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    #[tokio::test]
    async fn errors_are_json_with_a_stable_code() {
        let response =
            ApiError::conflict(codes::DEVICE_OFFLINE, "target device is offline").into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "application/json"
        );
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            body,
            r#"{"error":{"code":"DEVICE_OFFLINE","message":"target device is offline"}}"#
        );
    }

    #[test]
    fn the_common_errors_use_their_http_statuses() {
        assert_eq!(ApiError::unauthorized().status, StatusCode::UNAUTHORIZED);
        assert_eq!(ApiError::not_found().status, StatusCode::NOT_FOUND);
        assert_eq!(ApiError::invalid("x").status, StatusCode::BAD_REQUEST);
        assert_eq!(
            ApiError::internal("x").status,
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            ApiError::unauthorized().to_string(),
            "401 UNAUTHORIZED: sign in again"
        );
    }

    #[test]
    fn a_database_failure_becomes_an_opaque_internal_error() {
        let converted = ApiError::from(sqlx::Error::PoolClosed);
        assert_eq!(converted.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(converted.code, codes::INTERNAL);
        assert_eq!(converted.message, "a database operation failed");
    }
}
