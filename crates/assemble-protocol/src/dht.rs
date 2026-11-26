//! 仅用于网页超级管理员的 DHT 观测契约，不包含节点或 peer 地址。
use crate::primitives::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataStatus {
    Pending,
    Fetching,
    Deferred,
    Verified,
    NotFound,
    Invalid,
    Failed,
}

impl MetadataStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Fetching => "fetching",
            Self::Deferred => "deferred",
            Self::Verified => "verified",
            Self::NotFound => "not_found",
            Self::Invalid => "invalid",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DhtMode {
    Stopped,
    Collecting,
    Draining,
}
impl DhtMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Collecting => "collecting",
            Self::Draining => "draining",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DhtFile {
    pub path: Vec<String>,
    pub length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DhtObservation {
    pub id: String,
    pub first_seen: Timestamp,
    pub last_seen: Timestamp,
    pub announce_count: u64,
    pub sample_count: u64,
    pub status: MetadataStatus,
    pub name: Option<String>,
    pub total_size: Option<u64>,
    pub file_count: Option<u32>,
    pub fetched_at: Option<Timestamp>,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub magnet_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DhtDetails {
    #[serde(flatten)]
    pub observation: DhtObservation,
    pub files: Vec<DhtFile>,
    pub piece_length: Option<u64>,
    pub private: Option<bool>,
    pub comment: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DhtCounters {
    pub lookups: u64,
    pub lookups_with_peers: u64,
    pub fetch_attempts: u64,
    pub verified: u64,
    pub timeouts: u64,
    pub connection_errors: u64,
    pub protocol_errors: u64,
    pub validation_errors: u64,
    pub persistence_errors: u64,
    pub dropped_events: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DhtStatus {
    pub enabled: bool,
    pub mode: DhtMode,
    pub running: bool,
    pub last_error: Option<String>,
    pub total: u64,
    pub announces: u64,
    pub samples: u64,
    pub last_observed_at: Option<Timestamp>,
    pub statuses: std::collections::BTreeMap<String, u64>,
    pub queue_limit: u32,
    pub max_records: u64,
    pub workers: u32,
    pub metadata_bytes: u64,
    pub metadata_budget_bytes: u64,
    pub counters: DhtCounters,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DhtQuery {
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DhtControl {
    pub mode: DhtMode,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_contract_preserves_nullable_metadata_and_endpoint_free_status() {
        let sample: serde_json::Value =
            serde_json::from_str(include_str!("../../../apps/admin/contract/dht.json")).unwrap();
        let detail: DhtDetails = serde_json::from_value(sample["detail"].clone()).unwrap();
        let status: DhtStatus = serde_json::from_value(sample["status"].clone()).unwrap();
        assert_eq!(serde_json::to_value(detail).unwrap(), sample["detail"]);
        assert_eq!(serde_json::to_value(status).unwrap(), sample["status"]);
    }
}
