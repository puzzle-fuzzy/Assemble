//! 界面可以调用的具名命令。
//!
//! 界面没有通用的网络或文件能力，只能调用这里列出的几个操作。
//! 命令的错误只带稳定错误码，技术细节进日志，不回显给用户。

use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

use crate::session;
use crate::state::{AppState, LaunchState, launch_state};
use crate::tray;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: &'static str,
    /// 仅供诊断；界面不显示这段文字。
    pub message: String,
}

impl From<&session::SessionError> for CommandError {
    fn from(error: &session::SessionError) -> Self {
        Self {
            code: error.code(),
            message: error.to_string(),
        }
    }
}

/// 当前的完整状态。
#[tauri::command]
pub fn snapshot(app: AppHandle) -> crate::state::Snapshot {
    app.state::<AppState>().snapshot()
}

/// 在系统浏览器里登录。命令在登录流程结束（成功、取消或超时）后返回。
#[tauri::command]
pub async fn sign_in(app: AppHandle) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    match session::sign_in(&state).await {
        Ok(()) => Ok(state.snapshot()),
        Err(error) => {
            tracing::debug!(code = error.code(), %error, "sign-in did not complete");
            Err(CommandError::from(&error))
        }
    }
}

/// 取消进行中的登录。
#[tauri::command]
pub fn cancel_sign_in(app: AppHandle) {
    let state = app.state::<AppState>();
    if let Some(sign_in) = state.sign_in_attempt() {
        sign_in.cancel.cancel();
    }
}

/// 退出登录。服务端确认撤销之后，本机才忘掉设备会话。
#[tauri::command]
pub async fn sign_out(app: AppHandle) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    match session::sign_out(&state).await {
        Ok(()) => Ok(state.snapshot()),
        Err(error) => {
            tracing::debug!(code = error.code(), %error, "sign-out did not complete");
            Err(CommandError::from(&error))
        }
    }
}

/// 开机启动在系统里的实际状态。
#[tauri::command]
pub fn launch_at_login(app: AppHandle) -> LaunchState {
    launch_state(&app)
}

/// 设置开机启动，并读回系统里的实际状态。
#[tauri::command]
pub fn set_launch_at_login(app: AppHandle, enabled: bool) -> Result<LaunchState, CommandError> {
    if !crate::state::launch_at_login_supported() {
        return Err(CommandError {
            code: "UNSUPPORTED",
            message: "launch at login is only available in packaged builds".to_owned(),
        });
    }
    let result = if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    };
    if let Err(error) = result {
        tracing::warn!(%error, "launch at login could not be changed");
        return Err(CommandError {
            code: "SYSTEM_REJECTED",
            message: error.to_string(),
        });
    }
    let state = app.state::<AppState>();
    state.set_notice(None);
    tray::refresh(&app);
    state.broadcast();
    Ok(launch_state(&app))
}
