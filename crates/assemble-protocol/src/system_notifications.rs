//! 系统通知独立于日程与设备事件；内容只有具名类型，不承载诊断、路径或凭据。

use serde::{Deserialize, Serialize};

use crate::primitives::Timestamp;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SystemNotificationContent {
    StorageBudget {
        used_bytes: u64,
        budget_bytes: u64,
        warning_percent: u8,
    },
    StorageDisk {
        available_bytes: u64,
    },
    StorageUnavailable,
    TransferFailed {
        transfer_id: String,
        name: String,
        local_disk_full: bool,
    },
    DownloadFailed {
        task_id: String,
        name: String,
        local_disk_full: bool,
    },
    SessionExpired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SystemNotificationLevel {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemNotification {
    pub id: String,
    pub revision: u64,
    pub content: SystemNotificationContent,
    pub level: SystemNotificationLevel,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub resolved_at: Option<Timestamp>,
    pub read_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemNotifications {
    pub items: Vec<SystemNotification>,
    pub available: bool,
}

impl Default for SystemNotifications {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            available: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemNotificationRead {
    pub id: String,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemNotificationsRead {
    pub items: Vec<SystemNotificationRead>,
}
