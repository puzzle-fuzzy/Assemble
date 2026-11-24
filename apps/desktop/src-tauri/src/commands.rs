//! 界面可以调用的具名命令。
//!
//! 界面没有通用的网络或文件能力，只能调用这里列出的几个操作。
//! 命令的错误只带稳定错误码，技术细节进日志，不回显给用户。

use assemble_client::RequestOptions;
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;

use crate::session;
use crate::state::{AppState, LaunchState, launch_state};
use crate::tray;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    /// 稳定错误码：本机自己定的小写加下划线，或者服务端给的大写加下划线。
    pub code: String,
    /// 仅供诊断；界面不显示这段文字。
    pub message: String,
}

impl CommandError {
    fn local(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

impl From<&session::SessionError> for CommandError {
    fn from(error: &session::SessionError) -> Self {
        Self::local(error.code(), error.to_string())
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
        return Err(CommandError::local(
            "UNSUPPORTED",
            "launch at login is only available in packaged builds",
        ));
    }
    let result = if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    };
    if let Err(error) = result {
        tracing::warn!(%error, "launch at login could not be changed");
        return Err(CommandError::local("SYSTEM_REJECTED", error.to_string()));
    }
    let state = app.state::<AppState>();
    state.set_notice(None);
    tray::refresh(&app);
    Ok(launch_state(&app))
}

/// 给账户里的另一台设备改名。
#[tauri::command]
pub async fn rename_device(
    app: AppHandle,
    device_id: String,
    name: String,
) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let name = name.trim().to_owned();
    if name.is_empty() || name.chars().count() > assemble_protocol::device::DEVICE_NAME_MAX_UNITS {
        return Err(CommandError::local(
            "INVALID_INPUT",
            "the name is not acceptable",
        ));
    }
    let outcome = state
        .client
        .rename_device(
            &device_id,
            &assemble_protocol::device::RenameDeviceInput { name },
            &RequestOptions::new(&state.lifetime),
        )
        .await;
    finish_device_change(&state, outcome).await?;
    Ok(state.snapshot())
}

/// 移除账户里的另一台设备：它的登录随之撤销。移除自己请用"退出登录"。
#[tauri::command]
pub async fn remove_device(
    app: AppHandle,
    device_id: String,
) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    if state.device_id().as_deref() == Some(device_id.as_str()) {
        return Err(CommandError::local(
            "IS_THIS_DEVICE",
            "sign out instead of removing this device",
        ));
    }
    let outcome = state
        .client
        .remove_device(&device_id, &RequestOptions::new(&state.lifetime))
        .await;
    finish_device_change(&state, outcome).await?;
    Ok(state.snapshot())
}

/// 改动设备之后：失败就翻译成稳定错误码，成功就立刻拉一次最新列表。
async fn finish_device_change<T>(
    state: &AppState,
    outcome: Result<T, assemble_client::ClientError>,
) -> Result<(), CommandError> {
    match outcome {
        Ok(_) => {
            // 服务端也会经事件流推来新列表；这里不等它，界面马上就能看到结果。
            match state
                .client
                .devices(&RequestOptions::new(&state.lifetime))
                .await
            {
                Ok(devices) => {
                    state.set_devices(devices.iter().map(crate::state::DeviceView::from));
                }
                Err(error) if error.is_unauthorized() => session::revoked(state),
                Err(error) => {
                    tracing::debug!(%error, "the device list could not be refreshed");
                }
            }
            Ok(())
        }
        Err(error) => {
            if error.is_unauthorized() {
                session::revoked(state);
            }
            tracing::debug!(%error, "device change failed");
            Err(CommandError::from(&error))
        }
    }
}

/// 让用户挑一个接收文件夹。文件夹的位置由系统选择器直接交给 Rust，不经过界面。
///
/// 用户关掉选择器不算错误：什么都不改。
#[tauri::command]
pub async fn choose_receive_folder(app: AppHandle) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let (chosen, answer) = tokio::sync::oneshot::channel();
    let mut picker = app.dialog().file().set_title("选择接收文件夹");
    // 挂在主窗口上：它在窗口顶上以面板的形式出现，而不是另外弹一个窗口。
    if let Some(window) = app.get_webview_window(crate::state::MAIN_WINDOW) {
        picker = picker.set_parent(&window);
    }
    picker.pick_folder(move |folder| {
        // 关掉选择器、或者选中的不是一个本机文件夹时送出一个空值：
        // 接收这一侧据此知道"没有选"。
        let _ = chosen
            .send(folder.and_then(|folder| folder.as_path().map(std::path::Path::to_path_buf)));
    });

    let Some(folder) = answer.await.ok().flatten() else {
        return Ok(state.snapshot());
    };
    let validated = assemble_device::validate_directory(&folder)
        .await
        .map_err(|error| CommandError::local("INVALID_RECEIVE_FOLDER", error.to_string()))?;
    let settings = crate::state::Settings {
        receive_directory: Some(validated),
    };
    if let Err(error) = crate::receive::save_settings(&app, &settings) {
        tracing::warn!(%error, "the receive folder could not be saved");
        return Err(CommandError::local("SYSTEM_REJECTED", error.to_string()));
    }
    state.set_receive_folder(settings.receive_directory.as_deref());
    Ok(state.snapshot())
}

/// 回到默认的接收位置（下载/Assemble）。
#[tauri::command]
pub async fn reset_receive_folder(app: AppHandle) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let settings = crate::state::Settings {
        receive_directory: None,
    };
    if let Err(error) = crate::receive::save_settings(&app, &settings) {
        tracing::warn!(%error, "the receive folder could not be saved");
        return Err(CommandError::local("SYSTEM_REJECTED", error.to_string()));
    }
    state.set_receive_folder(None);
    Ok(state.snapshot())
}

/// 在访达或资源管理器里显示一个已经收到的文件。界面只给得出传输的标识。
#[tauri::command]
pub fn reveal_received(app: AppHandle, transfer_id: String) -> Result<(), CommandError> {
    let state = app.state::<AppState>();
    let Some(path) = state.saved_file(&transfer_id) else {
        return Err(CommandError::local(
            "NOT_SAVED",
            "this transfer has no file on this computer",
        ));
    };
    crate::notify::reveal(&app, path);
    Ok(())
}

/// 提示看过了，收起来。
#[tauri::command]
pub fn dismiss_notice(app: AppHandle) -> crate::state::Snapshot {
    let state = app.state::<AppState>();
    state.dismiss_notice();
    state.snapshot()
}

impl From<&assemble_client::ClientError> for CommandError {
    fn from(error: &assemble_client::ClientError) -> Self {
        use assemble_client::ClientError;
        let local = |code: &str| CommandError::local(code, error.to_string());
        match error {
            ClientError::InvalidRequest => local("INVALID_INPUT"),
            ClientError::InvalidResponse => local("INVALID_RESPONSE"),
            ClientError::Network | ClientError::Timeout => local("UNREACHABLE"),
            ClientError::Cancelled => local("CANCELLED"),
            ClientError::LocalSource | ClientError::TooLarge => local("LOCAL_SOURCE"),
            ClientError::Http {
                status,
                remote_code,
            } => match (status, remote_code.as_deref()) {
                (401, _) => local("UNAUTHORIZED"),
                (404, _) => local("NOT_FOUND"),
                // 服务端的稳定错误码都是大写加下划线；只有确实如此时才原样交给界面。
                (_, Some(code)) if is_server_code(code) => local(code),
                (status, _) => local(&format!("HTTP_{status}")),
            },
        }
    }
}

/// 服务端的稳定错误码的样子：不超过 64 个大写字母与下划线。
fn is_server_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_failures_become_stable_codes_without_echoing_details() {
        let http = |status: u16, remote_code: Option<&str>| assemble_client::ClientError::Http {
            status,
            remote_code: remote_code.map(ToOwned::to_owned),
        };
        let cases = [
            (http(401, Some("UNAUTHORIZED")), "UNAUTHORIZED"),
            (http(404, None), "NOT_FOUND"),
            (http(409, Some("DEVICE_OFFLINE")), "DEVICE_OFFLINE"),
            (http(500, None), "HTTP_500"),
            // 不像稳定错误码的东西不原样交给界面。
            (http(400, Some("bad code with spaces")), "HTTP_400"),
            (http(400, Some(&format!("X{}", "x".repeat(70)))), "HTTP_400"),
        ];
        for (error, expected) in cases {
            let converted = CommandError::from(&error);
            assert_eq!(converted.code, expected, "{error:?}");
            assert!(!converted.message.contains("bad code"));
        }
        assert_eq!(
            CommandError::from(&assemble_client::ClientError::Network).code,
            "UNREACHABLE"
        );
        assert_eq!(
            CommandError::from(&assemble_client::ClientError::InvalidRequest).code,
            "INVALID_INPUT"
        );
        assert_eq!(
            CommandError::from(&assemble_client::ClientError::TooLarge).code,
            "LOCAL_SOURCE"
        );
    }

    #[test]
    fn server_codes_are_uppercase_words() {
        assert!(is_server_code("QUOTA_EXCEEDED"));
        assert!(!is_server_code(""));
        assert!(!is_server_code("lowercase"));
        assert!(!is_server_code("WITH SPACE"));
    }
}
