//! 回收站只有本人资源和明确批次；修订值用于并发比较，不是授权凭据。
use crate::{drive_tree::Kind, primitives::Timestamp};
use serde::{Deserialize, Serialize};

pub const MAX_BATCH_ENTRIES: usize = 5000;
pub const MAX_BATCHES: i64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub kind: Kind,
    pub id: String,
    pub version: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub target: Target,
    pub name: String,
    pub file_count: u64,
    pub folder_count: u64,
    pub size: u64,
    pub revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Move {
    pub target: Target,
    pub revision: String,
    pub request_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch {
    pub id: String,
    pub root_id: String,
    pub kind: Kind,
    pub name: String,
    pub original_location: Vec<String>,
    pub deleted_at: Timestamp,
    pub version: i64,
    pub file_count: u64,
    pub folder_count: u64,
    pub size: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub items: Vec<Batch>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: Kind,
    pub name: String,
    pub size: Option<u64>,
    pub version: i64,
    pub created_at: Timestamp,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    pub batch: Batch,
    pub current: Item,
    pub ancestors: Vec<Item>,
    pub items: Vec<Item>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RestoreReason {
    Original,
    ParentUnavailable,
    DepthLimit,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Restored {
    pub batch_id: String,
    pub id: String,
    pub kind: Kind,
    pub name: String,
    pub parent_id: Option<String>,
    pub reason: RestoreReason,
    pub renamed: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestorePreview {
    #[serde(flatten)]
    pub restored: Restored,
    pub version: i64,
    pub revision: String,
    pub parent_location: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Restore {
    pub version: i64,
    pub revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Purged {
    pub id: String,
    pub logical_bytes_released: u64,
    pub cleanup_pending: bool,
}

/// 后缀保留，原名按 UTF-16 限额收短；不切断 Unicode 字符。
pub fn restored_name(name: &str, occupied: &[&str]) -> String {
    if !occupied.contains(&name) {
        return name.to_owned();
    }
    for index in 1..=occupied.len() + 1 {
        let suffix = if index == 1 {
            "（恢复）".to_owned()
        } else {
            format!("（恢复 {index}）")
        };
        let limit = crate::drive::FILE_NAME_MAX_UNITS - suffix.encode_utf16().count();
        let mut units = 0;
        let prefix: String = name
            .chars()
            .take_while(|c| {
                units += c.len_utf16();
                units <= limit
            })
            .collect();
        let candidate = format!("{prefix}{suffix}");
        if !occupied.contains(&candidate.as_str()) {
            return candidate;
        }
    }
    unreachable!("one more candidate than occupied names always fits")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restore_names_preserve_both_and_do_not_split_utf16() {
        assert_eq!(restored_name("资料", &[]), "资料");
        assert_eq!(
            restored_name("资料", &["资料", "资料（恢复）"]),
            "资料（恢复 2）"
        );
        let long = "🦀".repeat(127);
        let result = restored_name(&long, &[&long]);
        assert!(result.encode_utf16().count() <= 255);
        assert!(crate::drive::clean_file_name(&result).is_some());
    }
}
