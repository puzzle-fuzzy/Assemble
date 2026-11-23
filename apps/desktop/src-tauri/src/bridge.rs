//! 本机网关的接线：让 MCP 服务器这类本机工具能找到桌面端。
//!
//! 协议由 `assemble-device::gateway` 保证。这里只决定两件事：
//! 发现文件写在哪里，以及哪些操作在本构建里确实可用。
//! 没实现的操作如实回答"未提供"，对外也只公布真正可用的那几个。

use std::future::Future;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use assemble_device::gateway::{Gateway, GatewayBackend};
use assemble_protocol::gateway::{GatewayError, LocalCapability};
use assemble_protocol::primitives::Platform;
use serde_json::{Map, Value, json};
use tauri::{AppHandle, Manager};

use crate::state::{AppState, SessionView};

/// 本构建真正提供的操作，也是对外公布的全部操作。
/// 每实现一个新的操作，就在这里加一行。
const SUPPORTED: [(&str, bool); 3] = [
    ("capabilities", true),
    ("status", true),
    ("config.get", true),
];

const NOT_AVAILABLE: &str = "NOT_AVAILABLE";

pub struct Bridge {
    gateway: Mutex<Gateway>,
}

impl Bridge {
    /// 立即停止网关并删除发现文件。应用退出时调用。
    pub fn abort(&self) {
        self.gateway
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .abort();
    }

    fn url(&self) -> String {
        self.gateway
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .url
            .clone()
    }
}

/// 在后台启动网关，并把句柄交给应用管理。
///
/// 退出时的清理不由这里负责：框架托管的状态在进程结束前未必会被释放，
/// 所以应用的退出事件会显式调用 [`Bridge::abort`]。
pub fn start(app: AppHandle, bridge_path: &Path) {
    let bridge_path = bridge_path.to_path_buf();
    tauri::async_runtime::spawn(async move {
        let started = Gateway::start(
            &bridge_path,
            std::sync::Arc::new(BridgeBackend { app: app.clone() }),
        )
        .await;
        match started {
            Ok(gateway) => {
                tracing::info!(url = %gateway.url, "local bridge started");
                app.manage(Bridge {
                    gateway: Mutex::new(gateway),
                });
            }
            Err(error) => {
                tracing::error!(%error, "the local bridge could not be started");
            }
        }
    });
}

/// 对外公布的能力目录：恰好是本构建提供的操作。
fn advertised() -> Vec<LocalCapability> {
    SUPPORTED
        .iter()
        .map(|(operation, readonly)| LocalCapability {
            operation: (*operation).to_owned(),
            readonly: *readonly,
        })
        .collect()
}

/// "本构建没有提供这个操作"的拒绝。
fn not_available() -> GatewayError {
    GatewayError {
        code: NOT_AVAILABLE.to_owned(),
        message: "This operation is not available in this build".to_owned(),
    }
}

/// 本构建没有实现的操作：返回要带回的拒绝。
fn refuse_if_unsupported(operation: &str) -> Option<GatewayError> {
    SUPPORTED
        .iter()
        .all(|(provided, _)| *provided != operation)
        .then(not_available)
}

struct BridgeBackend {
    app: AppHandle,
}

impl BridgeBackend {
    fn state(&self) -> tauri::State<'_, AppState> {
        self.app.state::<AppState>()
    }

    fn connected(&self) -> bool {
        let state = self.state();
        matches!(state.session(), SessionView::SignedIn { .. })
            && state.snapshot().service.reachable == Some(true)
    }

    fn public_config(&self) -> Value {
        let state = self.state();
        let bridge_url = self.app.try_state::<Bridge>().map(|bridge| bridge.url());
        json!({
            "cloudUrl": state.client.deployment().api.as_str(),
            "deviceName": device_name(),
            "platform": Platform::current(),
            "roots": [],
            "connected": self.connected(),
            "bridgeUrl": bridge_url.unwrap_or_else(|| "http://127.0.0.1".to_owned()),
        })
    }

    fn dispatch(&self, operation: &str) -> Result<Value, GatewayError> {
        if let Some(refusal) = refuse_if_unsupported(operation) {
            return Err(refusal);
        }
        match operation {
            "status" => Ok(self.status()),
            "capabilities" => Ok(json!(advertised())),
            "config.get" => Ok(self.public_config()),
            // 在 `SUPPORTED` 里登记了却忘了接上处理：同样如实回答未提供。
            _ => Err(not_available()),
        }
    }
}

impl GatewayBackend for BridgeBackend {
    fn status(&self) -> Value {
        let state = self.state();
        let connected = self.connected();
        let mut status = Map::new();
        status.insert("connected".into(), connected.into());
        // 本构建还不接收设备命令，所以共享是关闭的。
        status.insert("sharingEnabled".into(), false.into());
        status.insert("deviceName".into(), device_name().into());
        status.insert("platform".into(), json!(Platform::current()));
        status.insert("roots".into(), json!([]));
        status.insert("tasks".into(), json!([]));
        status.insert("logs".into(), json!([]));
        // 可缺省的字段没有内容时不出现，不写 null。
        if state.snapshot().service.reachable == Some(false) {
            status.insert("error".into(), "Service unreachable".into());
        }
        Value::Object(status)
    }

    fn invoke(
        &self,
        operation: &str,
        _input: Map<String, Value>,
    ) -> impl Future<Output = Result<Value, GatewayError>> + Send {
        // 本构建提供的操作都是纯读取，结果可以立刻给出。
        std::future::ready(self.dispatch(operation))
    }
}

/// 本机的名字。系统环境里取不到时退回到命令，结果只取一次。
fn device_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let from_environment = ["HOSTNAME", "COMPUTERNAME"]
            .iter()
            .find_map(|variable| std::env::var(variable).ok())
            .filter(|name| !name.trim().is_empty());
        from_environment
            .or_else(|| {
                std::process::Command::new("hostname")
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            })
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Assemble".to_owned())
    })
}

#[cfg(test)]
mod tests {
    use assemble_protocol::gateway::OPERATIONS;

    use super::*;

    #[test]
    fn every_provided_operation_exists_in_the_contract() {
        for (provided, readonly) in SUPPORTED {
            assert!(
                OPERATIONS.contains(&(provided, readonly)),
                "{provided} is not in the contract with readonly={readonly}"
            );
        }
    }

    #[test]
    fn the_advertised_list_is_exactly_what_is_provided() {
        let advertised = advertised();
        assert_eq!(advertised.len(), SUPPORTED.len());
        for (capability, (operation, readonly)) in advertised.iter().zip(SUPPORTED) {
            assert_eq!(capability.operation, operation);
            assert_eq!(capability.readonly, readonly);
        }
    }

    #[test]
    fn every_other_contract_operation_is_refused_rather_than_faked() {
        let mut refused = 0;
        for (operation, _) in OPERATIONS {
            match refuse_if_unsupported(operation) {
                None => assert!(
                    SUPPORTED.iter().any(|(provided, _)| provided == operation),
                    "{operation} was neither provided nor refused"
                ),
                Some(refusal) => {
                    refused += 1;
                    assert_eq!(refusal.code, NOT_AVAILABLE);
                }
            }
        }
        // 实际运行时抓到过这个问题：对外公布了全部操作，却只有三个可用。
        assert_eq!(refused, OPERATIONS.len() - SUPPORTED.len());
    }

    #[test]
    fn unknown_operations_are_refused_too() {
        assert!(refuse_if_unsupported("status").is_none());
        assert!(refuse_if_unsupported("device.reboot").is_some());
    }
}
