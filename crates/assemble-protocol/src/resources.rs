//! 泛型资源：九种资源共用同一套记录封装与增删改查接口。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::primitives::{Platform, is_safe_relative_path, is_valid_id};

/// 一处显式地点：`label` 是可点击地名，`address` 是导航目的地。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub label: String,
    pub address: String,
}

/// 服务端保存的全部资源种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceName {
    Devices,
    Roots,
    Files,
    Reminders,
    Schedules,
    Downloads,
    Preferences,
    Notifications,
    Logs,
}

impl ResourceName {
    pub const ALL: [Self; 9] = [
        Self::Devices,
        Self::Roots,
        Self::Files,
        Self::Reminders,
        Self::Schedules,
        Self::Downloads,
        Self::Preferences,
        Self::Notifications,
        Self::Logs,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Devices => "devices",
            Self::Roots => "roots",
            Self::Files => "files",
            Self::Reminders => "reminders",
            Self::Schedules => "schedules",
            Self::Downloads => "downloads",
            Self::Preferences => "preferences",
            Self::Notifications => "notifications",
            Self::Logs => "logs",
        }
    }
}

/// `devices`：登记在账户下的一台电脑。`online` 与 `lastSeenAt` 只由服务端写入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceData {
    pub name: String,
    pub platform: Platform,
    pub sharing_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online: Option<bool>,
}

/// `roots`：云端的共享目录记录。它本身不授予本机访问权，必须与本机私有授权取交集。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootData {
    pub device_id: String,
    pub name: String,
    pub path: String,
    pub enabled: bool,
}

/// `files`：一个不可变的文件版本。`cloudAvailable` 为真表示服务端存有云副本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileData {
    pub name: String,
    pub size: u64,
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
    #[serde(default)]
    pub cloud_available: bool,
}

/// `reminders` 的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReminderStatus {
    Scheduled,
    Fired,
    Dismissed,
    Cancelled,
}

/// `reminders`：单点到点提醒，按自身时区在 `dueAt` 触发。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderData {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    pub due_at: String,
    pub timezone: String,
    pub status: ReminderStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// `schedules` 的重复规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    None,
    Daily,
    Weekly,
    Monthly,
}

/// `schedules` 的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleStatus {
    Upcoming,
    Done,
    Cancelled,
}

/// `schedules`：跨时段的日程，可按天、周、月重复到 `repeatUntil`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleData {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    pub start_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_at: Option<String>,
    pub repeat: Repeat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_until: Option<String>,
    pub status: ScheduleStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// `downloads` 的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Paused,
    Cancelled,
}

/// `downloads`：让某台在线设备把一个链接保存到指定共享目录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadData {
    pub name: String,
    pub url: String,
    pub device_id: String,
    pub root_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    pub status: DownloadStatus,
    pub progress: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `preferences`：按账户保存的一条键值偏好。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreferenceData {
    pub key: String,
    pub value: Value,
}

/// `notifications` 引用的语音待确认动作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTarget {
    pub recording_id: String,
    pub action_id: String,
}

/// `notifications`：投递给用户的一条通知，可回指提醒或日程。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationData {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    pub body: String,
    pub read: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_target: Option<VoiceTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reminder_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<String>,
}

/// `logs` 的级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// `logs`：设备上报的一条结构化日志。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogData {
    pub level: LogLevel,
    pub domain: String,
    pub message: String,
    pub context: Value,
}

/// 一条记录的 `data`，以 `resource` 字段区分种类。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "resource", content = "data", rename_all = "lowercase")]
pub enum ResourceData {
    Devices(DeviceData),
    Roots(RootData),
    Files(FileData),
    Reminders(ReminderData),
    Schedules(ScheduleData),
    Downloads(DownloadData),
    Preferences(PreferenceData),
    Notifications(NotificationData),
    Logs(LogData),
}

impl ResourceData {
    pub fn name(&self) -> ResourceName {
        match self {
            Self::Devices(_) => ResourceName::Devices,
            Self::Roots(_) => ResourceName::Roots,
            Self::Files(_) => ResourceName::Files,
            Self::Reminders(_) => ResourceName::Reminders,
            Self::Schedules(_) => ResourceName::Schedules,
            Self::Downloads(_) => ResourceName::Downloads,
            Self::Preferences(_) => ResourceName::Preferences,
            Self::Notifications(_) => ResourceName::Notifications,
            Self::Logs(_) => ResourceName::Logs,
        }
    }

    /// 按资源种类取出数据；种类不符时返回 `None`。
    pub fn of_name(&self, name: ResourceName) -> Option<&Self> {
        (self.name() == name).then_some(self)
    }
}

/// 一条资源记录：`version` 是乐观锁，更新与删除都必须带上读到的版本号。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JarvisRecord {
    pub id: String,
    pub version: u64,
    #[serde(flatten)]
    pub data: ResourceData,
    pub created_at: String,
    pub updated_at: String,
}

impl JarvisRecord {
    pub fn resource(&self) -> ResourceName {
        self.data.name()
    }
}

/// 列表接口的分页结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JarvisList {
    pub items: Vec<JarvisRecord>,
    pub total: u64,
}

/// 创建接口返回 `{ item }`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JarvisItem {
    pub item: JarvisRecord,
}

/// 列表查询参数：`limit` 为 1 到 100，默认 50。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

impl Default for ListQuery {
    fn default() -> Self {
        Self {
            search: None,
            limit: 50,
            offset: 0,
        }
    }
}

impl ListQuery {
    /// 按线上约束检查自身，越界时返回 `false`。
    pub fn is_valid(&self) -> bool {
        (1..=100).contains(&self.limit)
            && self
                .search
                .as_ref()
                .is_none_or(|search| search.encode_utf16().count() <= 1_000)
    }
}

/// 更新输入：带上读到的 `version`，只提交改动的字段；`unset` 清除可选字段。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInput {
    pub version: u64,
    pub data: serde_json::Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unset: Option<Vec<String>>,
}

impl UpdateInput {
    /// 至少要有改动或清除项，否则服务端会拒绝。
    pub fn is_valid(&self) -> bool {
        self.version > 0
            && (!self.data.is_empty() || self.unset.as_ref().is_some_and(|u| !u.is_empty()))
    }
}

/// 删除与读取单条记录时携带的版本号。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionInput {
    pub version: u64,
}

/// 删除资源成功时服务端的应答：`{ ok: true }`。
///
/// 注意它和账户接口的 `{ success: true }` 不是同一个结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OkResponse {
    pub ok: bool,
}

/// 共享目录路径的线上约束：1 到 4096 个字符且不含 NUL。
pub fn is_valid_root_path(path: &str) -> bool {
    !path.is_empty() && path.encode_utf16().count() <= 4_096 && !path.contains('\0')
}

/// 写入 `roots` 前检查各字段是否满足线上约束。
pub fn is_valid_root_data(root: &RootData) -> bool {
    is_valid_id(&root.device_id)
        && crate::primitives::is_text(&root.name, crate::primitives::TEXT_MAX_UNITS)
        && is_valid_root_path(&root.path)
}

/// 写入 `files`（含库存上报）前检查各字段是否满足线上约束。
pub fn is_valid_file_data(file: &FileData) -> bool {
    crate::primitives::is_text(&file.name, crate::primitives::TEXT_MAX_UNITS)
        && file.size < crate::primitives::MAX_FILE_SIZE
        && !file.mime_type.is_empty()
        && file.mime_type.encode_utf16().count() <= 255
        && crate::primitives::is_sha256_hex(&file.sha256)
        && file.device_id.as_deref().is_none_or(is_valid_id)
        && file.root_id.as_deref().is_none_or(is_valid_id)
        && file
            .relative_path
            .as_deref()
            .is_none_or(is_safe_relative_path)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn file_record() -> serde_json::Value {
        json!({
            "id": "file-1",
            "resource": "files",
            "version": 1,
            "createdAt": "2026-11-23T00:00:00.000Z",
            "updatedAt": "2026-11-23T00:00:00.000Z",
            "data": {
                "name": "笔记.txt",
                "size": 12,
                "mimeType": "text/plain",
                "sha256": SHA,
                "deviceId": "device-1",
                "rootId": "root-1",
                "relativePath": "docs/笔记.txt",
                "sourceModifiedAt": "2026-11-22T10:00:00.000Z",
                "cloudAvailable": true
            }
        })
    }

    #[test]
    fn record_data_is_tagged_by_the_resource_field() {
        let record: JarvisRecord = serde_json::from_value(file_record()).unwrap();
        assert_eq!(record.resource(), ResourceName::Files);
        assert_eq!(record.version, 1);
        let ResourceData::Files(file) = &record.data else {
            panic!("expected file data");
        };
        assert_eq!(file.relative_path.as_deref(), Some("docs/笔记.txt"));
        assert!(file.cloud_available);
    }

    #[test]
    fn record_round_trips_to_the_same_json_shape() {
        let record: JarvisRecord = serde_json::from_value(file_record()).unwrap();
        assert_eq!(serde_json::to_value(&record).unwrap(), file_record());
    }

    #[test]
    fn cloud_available_defaults_to_false_when_absent() {
        let mut value = file_record();
        value["data"]
            .as_object_mut()
            .unwrap()
            .remove("cloudAvailable");
        let record: JarvisRecord = serde_json::from_value(value).unwrap();
        let ResourceData::Files(file) = record.data else {
            panic!("expected file data");
        };
        assert!(!file.cloud_available);
    }

    #[test]
    fn unknown_fields_and_future_resources_are_handled_differently() {
        let mut value = file_record();
        value["data"]["futureField"] = json!(1);
        value["futureTopLevel"] = json!(true);
        let record: JarvisRecord = serde_json::from_value(value).unwrap();
        assert_eq!(record.resource(), ResourceName::Files);

        let mut unknown = file_record();
        unknown["resource"] = json!("holograms");
        assert!(serde_json::from_value::<JarvisRecord>(unknown).is_err());
    }

    #[test]
    fn data_must_match_the_declared_resource() {
        let mut mismatched = file_record();
        mismatched["resource"] = json!("reminders");
        assert!(serde_json::from_value::<JarvisRecord>(mismatched).is_err());
    }

    #[test]
    fn optional_fields_are_omitted_and_null_location_is_absent() {
        let record: JarvisRecord = serde_json::from_value(json!({
            "id": "reminder-1",
            "resource": "reminders",
            "version": 3,
            "createdAt": "2026-11-23T00:00:00.000Z",
            "updatedAt": "2026-11-23T00:00:00.000Z",
            "data": {
                "title": "周会",
                "location": null,
                "dueAt": "2026-11-24T09:00:00.000+08:00",
                "timezone": "Asia/Shanghai",
                "status": "scheduled"
            }
        }))
        .unwrap();
        let ResourceData::Reminders(reminder) = &record.data else {
            panic!("expected reminder data");
        };
        assert!(reminder.location.is_none());
        assert!(reminder.notes.is_none());
        let value = serde_json::to_value(&record).unwrap();
        let data = &value["data"];
        assert!(data.get("location").is_none());
        assert!(data.get("notes").is_none());
        assert_eq!(data["status"], "scheduled");
    }

    #[test]
    fn every_resource_name_round_trips() {
        for name in ResourceName::ALL {
            assert_eq!(
                serde_json::from_value::<ResourceName>(json!(name.as_str())).unwrap(),
                name
            );
        }
        assert!(serde_json::from_value::<ResourceName>(json!("nope")).is_err());
    }

    #[test]
    fn list_item_and_success_envelopes_match_the_wire_shape() {
        let list: JarvisList = serde_json::from_value(json!({
            "items": [file_record()],
            "total": 1
        }))
        .unwrap();
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.total, 1);

        let item: JarvisItem = serde_json::from_value(json!({ "item": file_record() })).unwrap();
        assert_eq!(item.item.resource(), ResourceName::Files);

        let deleted: OkResponse = serde_json::from_value(json!({ "ok": true })).unwrap();
        assert!(deleted.ok);
    }

    #[test]
    fn list_query_defaults_and_bounds_match_the_contract() {
        let query = serde_json::to_value(ListQuery::default()).unwrap();
        assert_eq!(query, json!({ "limit": 50, "offset": 0 }));
        assert!(ListQuery::default().is_valid());
        assert!(
            !ListQuery {
                limit: 0,
                ..ListQuery::default()
            }
            .is_valid()
        );
        assert!(
            !ListQuery {
                limit: 101,
                ..ListQuery::default()
            }
            .is_valid()
        );
    }

    #[test]
    fn update_requires_a_change_or_an_unset() {
        let mut input = UpdateInput {
            version: 1,
            data: serde_json::Map::new(),
            unset: None,
        };
        assert!(!input.is_valid());
        input.unset = Some(vec!["notes".into()]);
        assert!(input.is_valid());
        let value = serde_json::to_value(&UpdateInput {
            version: 2,
            data: serde_json::from_value(json!({ "title": "新标题" })).unwrap(),
            unset: None,
        })
        .unwrap();
        assert_eq!(
            value,
            json!({ "version": 2, "data": { "title": "新标题" } })
        );
    }

    #[test]
    fn file_validation_enforces_size_hash_and_path_rules() {
        let base: FileData = serde_json::from_value(json!({
            "name": "a.txt",
            "size": 1,
            "mimeType": "text/plain",
            "sha256": SHA
        }))
        .unwrap();
        assert!(is_valid_file_data(&base));

        let mut oversized = base.clone();
        oversized.size = crate::primitives::MAX_FILE_SIZE;
        assert!(!is_valid_file_data(&oversized));

        let mut uppercase = base.clone();
        uppercase.sha256 = SHA.to_uppercase();
        assert!(!is_valid_file_data(&uppercase));

        let mut escaping = base;
        escaping.relative_path = Some("../secret".into());
        assert!(!is_valid_file_data(&escaping));
    }

    #[test]
    fn root_validation_rejects_empty_and_unbounded_paths() {
        let root = RootData {
            device_id: "device-1".into(),
            name: "文档".into(),
            path: "/Users/example/Documents".into(),
            enabled: true,
        };
        assert!(is_valid_root_data(&root));
        assert!(!is_valid_root_data(&RootData {
            path: String::new(),
            ..root.clone()
        }));
        assert!(!is_valid_root_data(&RootData {
            path: "a\0b".into(),
            ..root
        }));
    }
}
