//! 管理端操作记录只投影安全的业务元数据，不含凭据或远控内容。
use crate::primitives::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActivityKind {
    File,
    Transfer,
    Media,
    Collection,
    Remote,
}

impl ActivityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Transfer => "transfer",
            Self::Media => "media",
            Self::Collection => "collection",
            Self::Remote => "remote",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ActivitySnapshot {
    File {
        name: String,
        size: u64,
        mime_type: Option<String>,
    },
    Transfer {
        name: String,
        size: u64,
        from_device_id: String,
        from_device_name: String,
        to_device_id: String,
        to_device_name: String,
        state: String,
        failure: Option<String>,
    },
    Media {
        title: String,
        scope: String,
    },
    Collection {
        title: String,
    },
    Remote {
        controller_id: String,
        controller_name: String,
        host_id: String,
        host_name: String,
        phase: String,
        end_reason: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityActor {
    pub account_id: Option<String>,
    pub name: Option<String>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySubject {
    pub kind: ActivityKind,
    pub id: String,
    pub name: Option<String>,
    pub owner_id: Option<String>,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminActivity {
    pub id: String,
    pub actor: ActivityActor,
    pub action: String,
    pub category: String,
    pub occurred_at: Timestamp,
    pub subject: Option<ActivitySubject>,
    pub snapshot: Option<ActivitySnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminActivityTarget {
    pub subject: ActivitySubject,
    pub snapshot: Option<ActivitySnapshot>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub events: Vec<AdminActivity>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admin_activity_matches_the_browser_contract_including_missing_legacy_fields() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../apps/admin/contract/activity.json"))
                .unwrap();
        let events: Vec<AdminActivity> = serde_json::from_value(fixture["events"].clone()).unwrap();
        let targets: Vec<AdminActivityTarget> =
            serde_json::from_value(fixture["targets"].clone()).unwrap();
        assert_eq!(serde_json::to_value(events).unwrap(), fixture["events"]);
        assert_eq!(serde_json::to_value(targets).unwrap(), fixture["targets"]);
        assert!(
            serde_json::from_value::<ActivitySnapshot>(serde_json::json!({
                "kind":"remote","controllerId":"a","controllerName":"A","hostId":"b",
                "hostName":"B","phase":"pending","endReason":null,"verification":"forbidden"
            }))
            .is_err()
        );
    }
}
