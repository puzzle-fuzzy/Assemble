//! 云盘上传会话；分片布局与位置依据不依赖网络或数据库。
use serde::{Deserialize, Serialize};

use crate::{
    drive::{clean_file_name, is_mime_type},
    drive_tree::{Folder, MAX_DEPTH, TreeFile, valid_parent},
    primitives::{MAX_FILE_SIZE, Timestamp, is_sha256_hex, is_valid_id},
};

pub const PART_BYTES: u64 = 2 * 1024 * 1024;
pub const DEVICE_LIMIT: i64 = 2;
pub const ACCOUNT_LIMIT: i64 = 4;
pub const GLOBAL_LIMIT: i64 = 32;
pub const REQUEST_LIMIT: i64 = 10_000;
pub const IDLE_MILLIS: i64 = 24 * 60 * 60 * 1000;
pub const LIFETIME_MILLIS: i64 = 7 * IDLE_MILLIS;

pub fn part_count(size: u64) -> Option<u32> {
    (size <= MAX_FILE_SIZE).then(|| u32::try_from(size.div_ceil(PART_BYTES)).unwrap_or(48))
}
pub fn part_size(size: u64, index: u32) -> Option<u64> {
    (index < part_count(size)?).then(|| (size - u64::from(index) * PART_BYTES).min(PART_BYTES))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderRevision {
    pub id: String,
    pub version: i64,
}
impl From<&Folder> for FolderRevision {
    fn from(folder: &Folder) -> Self {
        Self {
            id: folder.id.clone(),
            version: folder.version,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Create {
    pub request_id: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub mime_type: Option<String>,
    pub parent_id: String,
    /// 从根到当前目录，根目录本身不占一项；移动祖先也会使依据失效。
    pub location: Vec<FolderRevision>,
}
impl Create {
    pub fn valid(&self) -> bool {
        is_valid_id(&self.request_id)
            && clean_file_name(&self.name).is_some_and(|name| name == self.name)
            && self.size <= MAX_FILE_SIZE
            && is_sha256_hex(&self.sha256)
            && self.mime_type.as_deref().is_none_or(is_mime_type)
            && valid_parent(&self.parent_id)
            && self.location.len() <= MAX_DEPTH
            && self
                .location
                .iter()
                .all(|f| is_valid_id(&f.id) && f.version > 0)
            && self
                .location
                .last()
                .map_or(self.parent_id == "root", |f| f.id == self.parent_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Receiving,
    Completed,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub index: u32,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Upload {
    pub id: String,
    pub phase: Phase,
    pub part_bytes: u64,
    pub parts: Vec<Part>,
    pub expires_at: Timestamp,
    pub file: Option<TreeFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub part_bytes: u64,
    pub max_file_bytes: u64,
    pub device_limit: i64,
    pub account_limit: i64,
}
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            part_bytes: PART_BYTES,
            max_file_bytes: MAX_FILE_SIZE,
            device_limit: DEVICE_LIMIT,
            account_limit: ACCOUNT_LIMIT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_is_bounded_and_has_no_empty_last_part() {
        assert_eq!(part_count(0), Some(0));
        assert_eq!(part_size(0, 0), None);
        assert_eq!(part_size(PART_BYTES, 0), Some(PART_BYTES));
        assert_eq!(part_size(PART_BYTES, 1), None);
        assert_eq!(part_count(MAX_FILE_SIZE), Some(48));
        assert_eq!(part_count(MAX_FILE_SIZE + 1), None);
        for size in [0, 1, PART_BYTES, PART_BYTES + 1, MAX_FILE_SIZE] {
            assert_eq!(
                (0..part_count(size).unwrap())
                    .map(|i| part_size(size, i).unwrap())
                    .sum::<u64>(),
                size
            );
        }
    }
}
