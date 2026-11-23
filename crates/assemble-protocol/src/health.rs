//! 探活：`/health` 表示进程能应答，`/ready` 表示依赖已就绪。

use serde::{Deserialize, Serialize};

/// `GET /health` 的结果。公开、不缓存，任何会话状态下都能调用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
    pub version: String,
    pub timestamp: String,
    pub uptime: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Readiness {
    Ready,
    Unavailable,
}

/// `GET /ready` 的结果；不可用时服务端同时返回 503。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadinessResponse {
    pub status: Readiness,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn health_and_readiness_parse_the_public_shapes() {
        let health: HealthResponse = serde_json::from_value(json!({
            "status": "ok",
            "service": "api",
            "version": "0.1.0",
            "timestamp": "2026-11-23T12:00:00.000Z",
            "uptime": 12.5
        }))
        .unwrap();
        assert_eq!(health.service, "api");
        assert_eq!(health.uptime, 12.5);

        let ready: ReadinessResponse =
            serde_json::from_value(json!({ "status": "ready" })).unwrap();
        assert_eq!(ready.status, Readiness::Ready);
        let down: ReadinessResponse =
            serde_json::from_value(json!({ "status": "unavailable" })).unwrap();
        assert_eq!(down.status, Readiness::Unavailable);
    }
}
