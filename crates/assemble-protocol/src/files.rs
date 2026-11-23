//! 文件定位、上传元数据与设备库存上报。

use serde::{Deserialize, Serialize};

use crate::commands::DeviceCommand;
use crate::primitives::MAX_FILE_SIZE;
use crate::resources::{FileData, JarvisRecord};

/// 找文件的条件：`fileId`、`name`、`sha256` 至少给一个，其余用来缩小范围。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileResolveInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
}

impl FileResolveInput {
    /// 没有任何文件身份时服务端会拒绝，这里提前拦下。
    pub fn has_identity(&self) -> bool {
        self.file_id.is_some() || self.name.is_some() || self.sha256.is_some()
    }
}

/// 解析结果来自哪里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResolutionSource {
    /// 就在当前设备，`localPath` 可直接使用。
    Local,
    /// 服务端存有云副本，可以下载。
    Cloud,
    /// 只在另一台在线设备上；`command` 是让那台设备补传的任务。
    Remote,
    /// 哪里都找不到。
    Missing,
}

/// `POST /api/jarvis/files/resolve` 的结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileResolution {
    pub source: ResolutionSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<JarvisRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<DeviceCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// 上传时随文件一起提交的来源信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UploadMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_modified_at: Option<String>,
}

/// 库存里的一项：文件身份，不带 `cloudAvailable`（该字段不接受客户端提供）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryItem {
    pub name: String,
    pub size: u64,
    /// 省略时服务端按 `application/octet-stream` 处理。
    #[serde(
        default = "default_mime_type",
        skip_serializing_if = "is_default_mime_type"
    )]
    pub mime_type: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_modified_at: Option<String>,
}

const DEFAULT_MIME_TYPE: &str = "application/octet-stream";

fn default_mime_type() -> String {
    DEFAULT_MIME_TYPE.to_owned()
}

fn is_default_mime_type(value: &String) -> bool {
    value == DEFAULT_MIME_TYPE
}

impl InventoryItem {
    /// 上报前检查：大小、摘要、标识与相对路径都要满足线上约束。
    pub fn is_valid(&self) -> bool {
        let as_file = FileData {
            name: self.name.clone(),
            size: self.size,
            mime_type: self.mime_type.clone(),
            sha256: self.sha256.clone(),
            device_id: self.device_id.clone(),
            root_id: self.root_id.clone(),
            relative_path: self.relative_path.clone(),
            source_modified_at: self.source_modified_at.clone(),
            cloud_available: false,
        };
        crate::resources::is_valid_file_data(&as_file)
    }

    /// 超过上限的文件不能上云，扫描时据此直接跳过。
    pub fn is_oversized(&self) -> bool {
        self.size >= MAX_FILE_SIZE
    }
}

/// 库存上报：一次最多 1000 项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryInput {
    pub items: Vec<InventoryItem>,
}

/// 库存一次请求允许的最大项数。
pub const MAX_INVENTORY_ITEMS: usize = 1_000;

impl InventoryInput {
    pub fn is_valid(&self) -> bool {
        !self.items.is_empty()
            && self.items.len() <= MAX_INVENTORY_ITEMS
            && self.items.iter().all(InventoryItem::is_valid)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::primitives::{is_safe_relative_path, is_valid_id};

    #[test]
    fn resolve_input_requires_a_file_identity() {
        let empty = FileResolveInput::default();
        assert!(!empty.has_identity());
        assert!(
            FileResolveInput {
                sha256: Some("ab".into()),
                ..empty
            }
            .has_identity()
        );

        let value = serde_json::to_value(FileResolveInput {
            file_id: Some("file-1".into()),
            ..FileResolveInput::default()
        })
        .unwrap();
        assert_eq!(value, json!({ "fileId": "file-1" }));
        assert!(is_valid_id("file-1"));
    }

    #[test]
    fn resolution_distinguishes_local_cloud_remote_and_missing() {
        let local: FileResolution = serde_json::from_value(json!({
            "source": "local", "localPath": "/Users/example/Documents/a.txt"
        }))
        .unwrap();
        assert_eq!(local.source, ResolutionSource::Local);
        assert!(local.file.is_none());

        let missing: FileResolution =
            serde_json::from_value(json!({ "source": "missing", "message": "没有可用来源" }))
                .unwrap();
        assert_eq!(missing.source, ResolutionSource::Missing);
        assert_eq!(missing.message.as_deref(), Some("没有可用来源"));
    }

    #[test]
    fn inventory_items_omit_the_default_mime_type_and_reject_cloud_availability() {
        let item = InventoryItem {
            name: "a.txt".into(),
            size: 3,
            mime_type: default_mime_type(),
            sha256: "ab".repeat(32),
            device_id: Some("device-1".into()),
            root_id: Some("root-1".into()),
            relative_path: Some("a.txt".into()),
            source_modified_at: None,
        };
        assert!(item.is_valid());
        assert!(!item.is_oversized());
        let value = serde_json::to_value(&item).unwrap();
        assert!(value.get("mimeType").is_none());
        assert!(value.get("cloudAvailable").is_none());

        let parsed: InventoryItem = serde_json::from_value(json!({
            "name": "a.txt", "size": 3, "sha256": "ab"
        }))
        .unwrap();
        assert_eq!(parsed.mime_type, "application/octet-stream");
    }

    #[test]
    fn oversized_or_escaping_items_are_not_reported() {
        let mut oversized = InventoryItem {
            name: "big.bin".into(),
            size: MAX_FILE_SIZE,
            mime_type: default_mime_type(),
            sha256: "ab".repeat(32),
            device_id: None,
            root_id: None,
            relative_path: None,
            source_modified_at: None,
        };
        assert!(oversized.is_oversized());
        assert!(!oversized.is_valid());

        oversized.size = 1;
        oversized.relative_path = Some("../outside".into());
        assert!(!oversized.is_valid());
        assert!(!is_safe_relative_path("../outside"));
    }

    #[test]
    fn inventory_batch_must_be_non_empty_and_bounded() {
        let item = InventoryItem {
            name: "a.txt".into(),
            size: 3,
            mime_type: default_mime_type(),
            sha256: "ab".repeat(32),
            device_id: None,
            root_id: None,
            relative_path: None,
            source_modified_at: None,
        };
        assert!(!InventoryInput { items: vec![] }.is_valid());
        assert!(
            InventoryInput {
                items: vec![item.clone()]
            }
            .is_valid()
        );
        assert!(
            !InventoryInput {
                items: vec![item; MAX_INVENTORY_ITEMS + 1]
            }
            .is_valid()
        );
    }
}
