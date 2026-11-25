//! 影视库：共享投影、私人所有权、资料源与持久任务在服务端统一执行。

pub mod assets;
mod auth;
mod catalog;
mod collections;
pub mod jobs;
pub mod providers;
mod routes;
mod store;

pub use routes::routes;

use crate::config::ConfigError;
use assemble_store::secret::SecretString;

#[derive(Debug, Clone, Default)]
pub struct MediaConfig {
    pub tmdb_token: Option<SecretString>,
}
impl MediaConfig {
    pub fn from_lookup(get: &impl Fn(&'static str) -> Option<String>) -> Result<Self, ConfigError> {
        Ok(Self {
            tmdb_token: crate::config::secret(
                get,
                "ASSEMBLE_TMDB_TOKEN",
                "ASSEMBLE_TMDB_TOKEN_FILE",
            )?,
        })
    }
}

pub(crate) fn now() -> i64 {
    assemble_protocol::primitives::Timestamp::now().as_millis()
}
pub(crate) fn timestamp(value: i64) -> String {
    chrono::DateTime::from_timestamp_millis(value)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub(crate) fn id() -> Result<String, crate::error::ApiError> {
    crate::secrets::random_id()
        .map_err(|_| crate::error::ApiError::internal("identifier generation failed"))
}
pub(crate) fn spelling<T: serde::Serialize>(value: &T) -> String {
    crate::planner::spelling(value)
}
pub(crate) fn encode<T: serde::Serialize>(value: &T) -> Result<String, crate::error::ApiError> {
    serde_json::to_string(value)
        .map_err(|_| crate::error::ApiError::internal("media serialization failed"))
}
pub(crate) fn decode<T: serde::de::DeserializeOwned>(
    value: &str,
) -> Result<T, crate::error::ApiError> {
    serde_json::from_str(value)
        .map_err(|_| crate::error::ApiError::internal("stored media is invalid"))
}

#[cfg(test)]
mod tests;
