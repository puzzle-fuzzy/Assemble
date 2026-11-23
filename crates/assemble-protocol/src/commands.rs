//! 设备命令：服务端排队、设备领取并续租、设备回执。
//!
//! 实时性完全依靠 HTTP 长轮询：设备反复 `GET …/commands?waitSeconds=20`，
//! 领取后先上报 `running` 取得 60 秒租约，再做一次终态回执。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::resources::DownloadStatus;

/// 服务端明确告知命令历史已不存在时使用的错误码。
///
/// 只有它能让本机放弃一条等待确认的回执；普通 404 不能。
pub const COMMAND_NOT_FOUND: &str = "COMMAND_NOT_FOUND";

/// 长轮询等待秒数的上限，领取与单条命令观察共用。
pub const MAX_WAIT_SECONDS: u8 = 25;

/// 设备运行时实际使用的等待值：留出余量，使 30 秒的请求预算始终够用。
pub const DEVICE_WAIT_SECONDS: u8 = 20;

const _: () = assert!(DEVICE_WAIT_SECONDS <= MAX_WAIT_SECONDS);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl CommandStatus {
    /// 终态不会再变化；迟到的重复回执对终态命令是空操作。
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// 长轮询等待秒数：0 到 25。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct WaitSeconds(u8);

impl WaitSeconds {
    /// 超出线上允许范围时返回 `None`。
    pub fn new(seconds: u8) -> Option<Self> {
        (seconds <= MAX_WAIT_SECONDS).then_some(Self(seconds))
    }

    pub const fn none() -> Self {
        Self(0)
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

/// `upload`：把本机授权目录里的一个文件上云。`sha256` 存在时必须与文件内容一致。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadCommandInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    pub root_id: String,
    pub relative_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// `download`：把一个链接保存到本机授权目录。
///
/// 服务端把 `downloads` 记录原样复制进来，所以 `status` 与 `progress` 只是快照，
/// 本机以 `url`、`rootId` 与保存位置为准。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadCommandInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub root_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<DownloadStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
}

impl DownloadCommandInput {
    /// 保存位置：优先使用相对路径，否则以文件名直接放在共享目录下。
    pub fn target_relative_path(&self) -> Option<&str> {
        self.relative_path.as_deref().or(self.name.as_deref())
    }
}

/// `notify`：投递一条系统通知。`notificationId` 用于在本机去重。
///
/// 服务端把 `notifications` 记录展开进来，所以这里只是其中与投递和
/// 点击跳转有关的字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyCommandInput {
    pub notification_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reminder_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<String>,
}

/// `choose-root`：让本机弹出系统目录选择器；`name` 是建议的目录名。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChooseRootCommandInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// 一条命令的输入，以 `type` 字段区分。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "input", rename_all = "kebab-case")]
pub enum CommandInput {
    Upload(UploadCommandInput),
    Download(DownloadCommandInput),
    Notify(NotifyCommandInput),
    ChooseRoot(ChooseRootCommandInput),
    /// 壁纸命令在本阶段只需识别与拒绝，载荷保持原样。
    Wallpaper(Value),
}

impl CommandInput {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Upload(_) => "upload",
            Self::Download(_) => "download",
            Self::Notify(_) => "notify",
            Self::ChooseRoot(_) => "choose-root",
            Self::Wallpaper(_) => "wallpaper",
        }
    }
}

/// 服务端排队给本机的一条命令。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCommand {
    pub id: String,
    pub device_id: String,
    pub status: CommandStatus,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(flatten)]
    pub input: CommandInput,
    /// 只有壁纸命令在运行中带它；每次领取都是新的。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

/// 回执里的状态：`Running` 用来续租，其余是终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// 设备对一条命令的回执。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceReport {
    pub device_id: String,
    pub status: ReportStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DeviceReport {
    fn new(device_id: impl Into<String>, status: ReportStatus) -> Self {
        Self {
            device_id: device_id.into(),
            status,
            progress: None,
            claim_id: None,
            result: None,
            error: None,
        }
    }

    /// 领取命令：上报运行中以取得租约。
    pub fn running(device_id: impl Into<String>) -> Self {
        Self::new(device_id, ReportStatus::Running)
    }

    pub fn completed(device_id: impl Into<String>, result: Value) -> Self {
        Self {
            result: Some(result),
            ..Self::new(device_id, ReportStatus::Completed)
        }
    }

    /// 失败回执只带稳定的错误码，不带本机路径或系统诊断。
    pub fn failed(device_id: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            error: Some(code.into()),
            ..Self::new(device_id, ReportStatus::Failed)
        }
    }

    pub fn cancelled(device_id: impl Into<String>) -> Self {
        Self::new(device_id, ReportStatus::Cancelled)
    }

    /// 进度是 0 到 100；完成前最高只报 99。
    #[must_use]
    pub fn with_progress(mut self, progress: f64) -> Self {
        self.progress = Some(progress.clamp(0.0, 100.0));
        self
    }
}

/// `GET …/commands` 返回的命令列表。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandsResponse {
    pub items: Vec<DeviceCommand>,
}

/// 观察、回执与目录请求共用的单条命令封装。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandResponse {
    pub command: DeviceCommand,
}

/// 心跳：`sharingEnabled` 为假表示主动脱离；`reconnect` 表示在脱离后明确恢复。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatInput {
    pub sharing_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconnect: Option<bool>,
}

impl HeartbeatInput {
    /// 常规心跳不携带重连意图，远端断开后必须由用户明确恢复。
    pub fn sharing() -> Self {
        Self {
            sharing_enabled: true,
            reconnect: None,
        }
    }
}

/// 请求某台设备选择目录时可带上的建议名称。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryRequestInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// 目录请求的结果：建立了共享目录，或用户取消。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub enum DirectoryRequestResult {
    Root { root_id: String },
    Cancelled { cancelled: bool },
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn command(kind: &str, input: serde_json::Value) -> serde_json::Value {
        json!({
            "id": "cmd-1",
            "deviceId": "device-1",
            "type": kind,
            "status": "queued",
            "createdAt": "2026-11-23T00:00:00.000Z",
            "updatedAt": "2026-11-23T00:00:00.000Z",
            "input": input
        })
    }

    #[test]
    fn every_command_kind_is_recognised_with_its_input() {
        let cases = [
            (
                command(
                    "upload",
                    json!({ "rootId": "root-1", "relativePath": "a.txt" }),
                ),
                "upload",
            ),
            (
                command(
                    "download",
                    json!({ "downloadId": "d-1", "name": "a.zip", "url": "https://example.com/a.zip",
                             "deviceId": "device-1", "rootId": "root-1", "status": "queued", "progress": 0.0 }),
                ),
                "download",
            ),
            (
                command(
                    "notify",
                    json!({ "notificationId": "n-1", "title": "周会", "body": "9 点" }),
                ),
                "notify",
            ),
            (
                command("choose-root", json!({ "name": "文档" })),
                "choose-root",
            ),
            (command("wallpaper", json!({ "opaque": true })), "wallpaper"),
        ];
        for (value, kind) in cases {
            let parsed: DeviceCommand = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(parsed.input.kind(), kind);
            assert_eq!(parsed.status, CommandStatus::Queued);
            assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
        }
    }

    #[test]
    fn upload_input_exposes_its_authorisation_scope() {
        let parsed: DeviceCommand = serde_json::from_value(command(
            "upload",
            json!({ "fileId": "file-1", "rootId": "root-1", "relativePath": "a.txt", "sha256": "ab" }),
        ))
        .unwrap();
        let CommandInput::Upload(upload) = parsed.input else {
            panic!("expected upload");
        };
        assert_eq!(upload.root_id, "root-1");
        assert_eq!(upload.relative_path, "a.txt");
        assert_eq!(upload.sha256.as_deref(), Some("ab"));
    }

    #[test]
    fn download_falls_back_to_the_file_name_when_no_relative_path() {
        let parse = |input: serde_json::Value| {
            let parsed: DeviceCommand = serde_json::from_value(command("download", input)).unwrap();
            let CommandInput::Download(download) = parsed.input else {
                panic!("expected download");
            };
            download
        };
        let nested = parse(json!({ "name": "a.zip", "url": "https://e.com/a.zip", "rootId": "r" }));
        assert_eq!(nested.target_relative_path(), Some("a.zip"));
        let explicit = parse(json!({
            "name": "a.zip", "url": "https://e.com/a.zip", "rootId": "r", "relativePath": "sub/a.zip"
        }));
        assert_eq!(explicit.target_relative_path(), Some("sub/a.zip"));
        let missing = parse(json!({ "url": "https://e.com/a.zip", "rootId": "r" }));
        assert_eq!(missing.target_relative_path(), None);
    }

    #[test]
    fn an_unknown_command_kind_is_rejected_rather_than_executed() {
        let value = command("reboot", json!({}));
        assert!(serde_json::from_value::<DeviceCommand>(value).is_err());
    }

    #[test]
    fn reports_carry_only_the_fields_that_apply() {
        let running = serde_json::to_value(DeviceReport::running("device-1")).unwrap();
        assert_eq!(
            running,
            json!({ "deviceId": "device-1", "status": "running" })
        );

        let failed =
            serde_json::to_value(DeviceReport::failed("device-1", "ROOT_NOT_AUTHORIZED")).unwrap();
        assert_eq!(
            failed,
            json!({ "deviceId": "device-1", "status": "failed", "error": "ROOT_NOT_AUTHORIZED" })
        );

        let done = serde_json::to_value(
            DeviceReport::completed("device-1", json!({ "rootId": "root-1" })).with_progress(140.0),
        )
        .unwrap();
        assert_eq!(done["progress"], 100.0);
        assert_eq!(done["result"]["rootId"], "root-1");
    }

    #[test]
    fn terminal_statuses_are_identified() {
        assert!(!CommandStatus::Queued.is_terminal());
        assert!(!CommandStatus::Running.is_terminal());
        assert!(CommandStatus::Completed.is_terminal());
        assert!(CommandStatus::Failed.is_terminal());
        assert!(CommandStatus::Cancelled.is_terminal());
    }

    #[test]
    fn wait_seconds_is_bounded_by_the_server() {
        assert_eq!(WaitSeconds::new(25).map(WaitSeconds::get), Some(25));
        assert_eq!(WaitSeconds::new(26), None);
        assert_eq!(serde_json::to_value(WaitSeconds::none()).unwrap(), json!(0));
    }

    #[test]
    fn envelopes_heartbeat_and_directory_results_match_the_wire() {
        let commands: CommandsResponse = serde_json::from_value(json!({
            "items": [command("notify", json!({ "notificationId": "n", "title": "t" }))]
        }))
        .unwrap();
        assert_eq!(commands.items.len(), 1);

        let heartbeat = serde_json::to_value(HeartbeatInput::sharing()).unwrap();
        assert_eq!(heartbeat, json!({ "sharingEnabled": true }));

        let root: DirectoryRequestResult =
            serde_json::from_value(json!({ "rootId": "root-1" })).unwrap();
        assert_eq!(
            root,
            DirectoryRequestResult::Root {
                root_id: "root-1".into()
            }
        );
        let cancelled: DirectoryRequestResult =
            serde_json::from_value(json!({ "cancelled": true })).unwrap();
        assert_eq!(
            cancelled,
            DirectoryRequestResult::Cancelled { cancelled: true }
        );
    }
}
