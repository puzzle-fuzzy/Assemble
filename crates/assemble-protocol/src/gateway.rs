//! 本机网关：MCP 服务器经回环端口调用桌面端的操作目录。
//!
//! 这是必须保持兼容的外部接口。现有的 `apps/mcp` 通过 `bridge.json` 找到
//! 网关，再按这里的操作名发起 RPC；改名或改语义都会让 `jarvis_*` 工具失效。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::DeviceCommand;
use crate::primitives::Platform;
use crate::resources::LogData;

/// 一条操作及其是否只读。顺序与 `localOperationNames` 一致。
type Operation = (&'static str, bool);

const READ: bool = true;
const WRITE: bool = false;

/// 全部操作名，按契约顺序排列；第二项表示该操作是否只读。
pub const OPERATIONS: &[Operation] = &[
    // 规划网关
    ("planning.capabilities", READ),
    ("planning.goals.list", READ),
    ("planning.goals.get", READ),
    ("planning.goals.create", WRITE),
    ("planning.goals.update", WRITE),
    ("planning.goals.remove", WRITE),
    ("planning.tasks.list", READ),
    ("planning.tasks.get", READ),
    ("planning.tasks.create", WRITE),
    ("planning.tasks.update", WRITE),
    ("planning.tasks.transition", WRITE),
    ("planning.tasks.assign", WRITE),
    ("planning.tasks.dependencies", WRITE),
    ("planning.tasks.remove", WRITE),
    ("planning.workBlocks.list", READ),
    ("planning.workBlocks.get", READ),
    ("planning.workBlocks.lock", WRITE),
    ("planning.plans.list", READ),
    ("planning.plans.get", READ),
    ("planning.plans.propose", WRITE),
    ("planning.plans.update", WRITE),
    ("planning.plans.preview", READ),
    ("planning.plans.apply", WRITE),
    ("planning.plans.reject", WRITE),
    ("planning.plans.remove", WRITE),
    // 生产力网关
    ("productivity.capabilities", READ),
    ("productivity.statements.create", WRITE),
    ("productivity.statements.get", READ),
    ("productivity.statements.remove", WRITE),
    ("productivity.notes.list", READ),
    ("productivity.notes.get", READ),
    ("productivity.notes.create", WRITE),
    ("productivity.notes.update", WRITE),
    ("productivity.notes.archive", WRITE),
    ("productivity.notes.remove", WRITE),
    ("context.briefing", READ),
    ("context.search", READ),
    // 壁纸网关
    ("wallpapers.capabilities", READ),
    ("wallpapers.list", READ),
    ("wallpapers.get", READ),
    ("wallpapers.import", WRITE),
    ("wallpapers.update", WRITE),
    ("wallpapers.remove", WRITE),
    ("wallpapers.devices", READ),
    ("wallpapers.apply", WRITE),
    ("wallpapers.pause", WRITE),
    ("wallpapers.resume", WRITE),
    ("wallpapers.stop", WRITE),
    ("wallpapers.command.get", READ),
    // 影视网关
    ("media.capabilities", READ),
    ("media.metadata.search", READ),
    ("media.catalog.details", READ),
    ("media.actors.credits", READ),
    ("media.collections.list", READ),
    ("media.collections.get", READ),
    ("media.collections.management.get", READ),
    ("media.collections.create", WRITE),
    ("media.collections.update", WRITE),
    ("media.collections.delete", WRITE),
    ("media.entries.collection", READ),
    ("media.showcase.list", READ),
    ("media.public.list", READ),
    ("media.public.get", READ),
    ("media.public.create", WRITE),
    ("media.public.update", WRITE),
    ("media.public.delete", WRITE),
    ("media.public.import", WRITE),
    ("media.public.job.get", READ),
    ("media.public.gallery.list", READ),
    ("media.public.gallery.sync", WRITE),
    ("media.public.gallery.import", WRITE),
    ("media.public.poster.select", WRITE),
    ("media.private.list", READ),
    ("media.private.get", READ),
    ("media.private.create", WRITE),
    ("media.private.update", WRITE),
    ("media.private.delete", WRITE),
    ("media.private.import", WRITE),
    ("media.private.job.get", READ),
    ("media.private.gallery.list", READ),
    ("media.private.gallery.sync", WRITE),
    ("media.private.gallery.import", WRITE),
    ("media.private.poster.select", WRITE),
    // 语音网关
    ("voice.capabilities", READ),
    ("voice.recordings.list", READ),
    ("voice.recordings.get", READ),
    ("voice.recordings.claim", WRITE),
    ("voice.recordings.renew", WRITE),
    ("voice.recordings.propose", WRITE),
    ("voice.recordings.complete", WRITE),
    ("voice.actions.apply", WRITE),
    // 设备与资源
    ("capabilities", READ),
    ("status", READ),
    ("config.get", READ),
    ("config.update", WRITE),
    ("device.connect", WRITE),
    ("device.disconnect", WRITE),
    ("roots.add", WRITE),
    ("roots.remove", WRITE),
    ("roots.choose", WRITE),
    ("roots.list", READ),
    ("roots.update", WRITE),
    ("files.resolve", READ),
    ("files.fetch", WRITE),
    ("files.scan", WRITE),
    ("files.upload", WRITE),
    ("downloads.start", WRITE),
    ("downloads.list", READ),
    ("downloads.cancel", WRITE),
    ("resources.list", READ),
    ("resources.get", READ),
    ("resources.create", WRITE),
    ("resources.update", WRITE),
    ("resources.delete", WRITE),
    ("cloud.request", WRITE),
];

/// 操作是否在契约目录里，以及它是否只读；未知操作返回 `None`。
pub fn operation_readonly(name: &str) -> Option<bool> {
    OPERATIONS
        .iter()
        .find(|(operation, _)| *operation == name)
        .map(|(_, readonly)| *readonly)
}

/// 网关对外公布的操作清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalCapability {
    pub operation: String,
    pub readonly: bool,
}

/// 全部操作及其只读标记，顺序与契约一致。
pub fn capabilities() -> Vec<LocalCapability> {
    OPERATIONS
        .iter()
        .map(|(operation, readonly)| LocalCapability {
            operation: (*operation).to_owned(),
            readonly: *readonly,
        })
        .collect()
}

/// 本机当前认可的一个共享目录（云端记录与本机授权的交集）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRoot {
    pub id: String,
    pub name: String,
    pub path: String,
    pub enabled: bool,
}

/// `GET /status` 与 `config.get` 共用的本机状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalStatus {
    pub connected: bool,
    pub sharing_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub device_name: String,
    pub platform: Platform,
    pub roots: Vec<LocalRoot>,
    /// 正在执行或等待云端确认的命令。
    pub tasks: Vec<DeviceCommand>,
    pub logs: Vec<LogData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_scan_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `POST /rpc` 的请求体：操作名加上不透明的输入对象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalRpcRequest {
    pub operation: String,
    pub input: serde_json::Map<String, Value>,
}

/// 网关与 MCP 之间的错误：稳定的错误码加一句可读说明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayError {
    pub code: String,
    pub message: String,
}

/// 每个网关接口的响应：成功为 `{ result }`，失败为 `{ error: { code, message } }`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GatewayEnvelope<T> {
    Ok { result: T },
    Err { error: GatewayError },
}

/// 网关一次请求允许的最大正文：1 MiB。
pub const MAX_GATEWAY_BODY_BYTES: usize = 1_048_576;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// 契约里的操作总数：规划 25、生产力 12、壁纸 12、影视 35、语音 8、基础 23。
    const EXPECTED_OPERATIONS: usize = 25 + 12 + 12 + 35 + 8 + 23;

    #[test]
    fn the_catalog_has_no_duplicates_and_covers_every_domain() {
        assert_eq!(OPERATIONS.len(), EXPECTED_OPERATIONS);
        let mut names: Vec<_> = OPERATIONS.iter().map(|(name, _)| *name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "duplicate operation names");
    }

    #[test]
    fn readonly_flags_match_the_contract() {
        for name in [
            "capabilities",
            "status",
            "config.get",
            "roots.list",
            "downloads.list",
            "resources.list",
            "resources.get",
            "files.resolve",
            "planning.plans.preview",
            "context.briefing",
            "wallpapers.command.get",
            "media.public.job.get",
            "voice.recordings.get",
        ] {
            assert_eq!(operation_readonly(name), Some(true), "{name}");
        }
        for name in [
            "files.fetch",
            "files.upload",
            "config.update",
            "device.connect",
            "cloud.request",
            "planning.plans.apply",
            "planning.workBlocks.lock",
            "productivity.notes.archive",
            "wallpapers.apply",
            "media.public.import",
            "voice.actions.apply",
        ] {
            assert_eq!(operation_readonly(name), Some(false), "{name}");
        }
        assert_eq!(operation_readonly("device.reboot"), None);
    }

    #[test]
    fn capabilities_project_the_catalog() {
        let listed = capabilities();
        assert_eq!(listed.len(), OPERATIONS.len());
        assert_eq!(listed[0].operation, "planning.capabilities");
        assert!(listed[0].readonly);
        assert_eq!(listed.last().unwrap().operation, "cloud.request");
    }

    #[test]
    fn envelopes_match_the_bridge_wire_format() {
        let ok: GatewayEnvelope<u8> = serde_json::from_value(json!({ "result": 7 })).unwrap();
        assert_eq!(ok, GatewayEnvelope::Ok { result: 7 });
        let err: GatewayEnvelope<u8> = serde_json::from_value(json!({
            "error": { "code": "UNAUTHORIZED", "message": "Local bridge authentication required" }
        }))
        .unwrap();
        assert_eq!(
            err,
            GatewayEnvelope::Err {
                error: GatewayError {
                    code: "UNAUTHORIZED".into(),
                    message: "Local bridge authentication required".into(),
                }
            }
        );
        assert_eq!(
            serde_json::to_value(GatewayEnvelope::Err::<u8> {
                error: GatewayError {
                    code: "NOT_READY".into(),
                    message: "Device runtime is not ready".into(),
                }
            })
            .unwrap(),
            json!({ "error": { "code": "NOT_READY", "message": "Device runtime is not ready" } })
        );
    }

    #[test]
    fn status_omits_absent_timestamps_and_error() {
        let status = LocalStatus {
            connected: false,
            sharing_enabled: false,
            device_id: None,
            device_name: "example-host".into(),
            platform: Platform::Darwin,
            roots: vec![],
            tasks: vec![],
            logs: vec![],
            last_heartbeat_at: None,
            last_scan_at: None,
            error: None,
        };
        let value = serde_json::to_value(&status).unwrap();
        assert_eq!(
            value,
            json!({
                "connected": false,
                "sharingEnabled": false,
                "deviceName": "example-host",
                "platform": "darwin",
                "roots": [],
                "tasks": [],
                "logs": []
            })
        );
    }

    #[test]
    fn rpc_request_pairs_an_operation_with_an_opaque_object() {
        let request: LocalRpcRequest = serde_json::from_value(json!({
            "operation": "roots.list",
            "input": { "unused": [1, 2] }
        }))
        .unwrap();
        assert_eq!(request.operation, "roots.list");
        assert_eq!(request.input["unused"], json!([1, 2]));
        assert!(operation_readonly(&request.operation) == Some(true));
    }
}
