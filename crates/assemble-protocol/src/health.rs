//! 探活：不需要登录。

use serde::{Deserialize, Serialize};

/// 进程能应答。`GET /healthz`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: String,
}

/// 数据库可用。`GET /readyz`；不可用时服务端回答 503。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessResponse {
    pub status: String,
}

pub const HEALTH_PATH: &str = "/healthz";
pub const READY_PATH: &str = "/readyz";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_and_readiness_are_plain_status_objects() {
        let health: HealthResponse = serde_json::from_str(r#"{"status":"ok"}"#).unwrap();
        let ready: ReadinessResponse =
            serde_json::from_str(r#"{"status":"ok","extra":1}"#).unwrap();
        assert_eq!(health.status, "ok");
        assert_eq!(ready.status, "ok");
        assert_eq!(HEALTH_PATH, "/healthz");
        assert_eq!(READY_PATH, "/readyz");
    }
}
