//! 界面可以调用的具名命令。
//!
//! 界面没有通用的网络或文件能力，只能调用这里列出的几个操作。
//! 命令的错误只带稳定错误码，技术细节进日志，不回显给用户。

use assemble_client::RequestOptions;
use assemble_protocol::memo::{Memo, MemoCreate, MemoList, MemoQuery, MemoUpdate};
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;
use tokio::io::AsyncReadExt;

use crate::registry::Destination;
use crate::session;
use crate::state::{AppState, LaunchState, launch_state};
use crate::tray;

fn remote_window(
    window: &tauri::WebviewWindow,
    state: &AppState,
) -> Result<MediaCall, CommandError> {
    if window.label() != crate::state::MAIN_WINDOW || state.device_id().is_none() {
        return Err(CommandError::local(
            "FORBIDDEN",
            "remote desktop requires the signed-in main window",
        ));
    }
    Ok(MediaCall::new(state))
}

#[tauri::command]
pub fn remote_fullscreen(
    app: AppHandle,
    window: tauri::WebviewWindow,
    enabled: bool,
) -> Result<(), CommandError> {
    if window.label() != crate::state::MAIN_WINDOW {
        return Err(CommandError::local(
            "FORBIDDEN",
            "fullscreen requires the main window",
        ));
    }
    if enabled {
        remote_window(&window, &app.state::<AppState>())?;
    }
    window
        .set_fullscreen(enabled)
        .map_err(|_| CommandError::local("WINDOW_UNAVAILABLE", "fullscreen could not be changed"))
}

#[tauri::command]
pub async fn remote_request(
    app: AppHandle,
    window: tauri::WebviewWindow,
    host_id: String,
) -> Result<assemble_protocol::remote::RemoteSession, CommandError> {
    let state = app.state::<AppState>();
    let call = remote_window(&window, &state)?;
    call.finish(
        &state,
        state
            .client
            .remote_request(
                &assemble_protocol::remote::RemoteRequest { host_id },
                &crate::remote::options(&call.cancel),
            )
            .await,
    )
}
#[tauri::command]
pub async fn remote_approve(
    app: AppHandle,
    window: tauri::WebviewWindow,
    id: String,
) -> Result<assemble_protocol::remote::RemoteSession, CommandError> {
    let state = app.state::<AppState>();
    let call = remote_window(&window, &state)?;
    let result = crate::remote::approve(&app, id).await;
    if call.cancel.is_cancelled() {
        return Err(CommandError::local("CANCELLED", "account session changed"));
    }
    result
}
#[tauri::command]
pub async fn remote_end(
    app: AppHandle,
    window: tauri::WebviewWindow,
    id: String,
) -> Result<(), CommandError> {
    let state = app.state::<AppState>();
    let call = remote_window(&window, &state)?;
    state.remote_runtime.stop(Some(&id)).await;
    call.finish(
        &state,
        state
            .client
            .remote_end(&id, &crate::remote::options(&call.cancel))
            .await,
    )
}
#[tauri::command]
pub async fn remote_poll(
    app: AppHandle,
    window: tauri::WebviewWindow,
    id: String,
    frame_after: u32,
) -> Result<assemble_protocol::remote::RemotePoll, CommandError> {
    let state = app.state::<AppState>();
    let call = remote_window(&window, &state)?;
    let result = call.finish(
        &state,
        state
            .client
            .remote_poll(&id, 0, frame_after, &crate::remote::options(&call.cancel))
            .await,
    )?;
    if state.device_id().as_deref() != Some(result.session.controller_id.as_str()) {
        return Err(CommandError::local(
            "FORBIDDEN",
            "only the controller may view frames",
        ));
    }
    Ok(result)
}
#[tauri::command]
pub async fn remote_audio(
    app: AppHandle,
    window: tauri::WebviewWindow,
    id: String,
    after: u32,
) -> Result<assemble_protocol::remote::AudioPlayback, CommandError> {
    let state = app.state::<AppState>();
    let call = remote_window(&window, &state)?;
    let audio = call.finish(
        &state,
        state
            .client
            .remote_audio_poll(&id, after, &crate::remote::options(&call.cancel))
            .await,
    )?;
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        state.remote_runtime.decode_audio(&id, audio)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = audio;
        Err(CommandError::local(
            "REMOTE_UNSUPPORTED",
            "audio requires Windows or macOS",
        ))
    }
}
#[tauri::command]
pub async fn remote_clipboard(
    app: AppHandle,
    window: tauri::WebviewWindow,
    id: String,
    direction: assemble_protocol::remote::ClipboardDirection,
    paste: bool,
) -> Result<assemble_protocol::remote::ClipboardStatus, CommandError> {
    use assemble_protocol::remote::{
        ClipboardDirection, ClipboardOperation, ClipboardStatus, RemotePhase, valid_clipboard,
    };
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let state = app.state::<AppState>();
    let call = remote_window(&window, &state)?;
    let view = call.finish(
        &state,
        state
            .client
            .remote_sessions(&crate::remote::options(&call.cancel))
            .await,
    )?;
    if !view.sessions.iter().any(|s| {
        s.id == id
            && s.phase == RemotePhase::Active
            && Some(s.controller_id.clone()) == state.device_id()
    }) {
        return Err(CommandError::local(
            "REMOTE_ENDED",
            "no active controller session",
        ));
    }
    let operation = match direction {
        ClipboardDirection::ToHost => {
            let Ok(text) = app.clipboard().read_text() else {
                return Ok(ClipboardStatus::Unavailable);
            };
            if !valid_clipboard(&text) {
                return Ok(ClipboardStatus::TooLarge);
            }
            ClipboardOperation::Write { text, paste }
        }
        ClipboardDirection::FromHost => {
            if paste {
                return Err(CommandError::local(
                    "INVALID_INPUT",
                    "paste direction is invalid",
                ));
            }
            ClipboardOperation::Read
        }
    };
    let result = call.finish(
        &state,
        state
            .client
            .remote_clipboard_exchange(&id, operation, &crate::remote::options(&call.cancel))
            .await,
    )?;
    if result.status == ClipboardStatus::Completed && direction == ClipboardDirection::FromHost {
        let Some(text) = result.text else {
            return Err(CommandError::local(
                "INVALID_STATE",
                "missing clipboard result",
            ));
        };
        if app.clipboard().write_text(text).is_err() {
            return Ok(ClipboardStatus::Unavailable);
        }
    }
    Ok(result.status)
}
#[tauri::command]
pub async fn remote_inputs(
    app: AppHandle,
    window: tauri::WebviewWindow,
    id: String,
    events: Vec<assemble_protocol::remote::SequencedInput>,
) -> Result<(), CommandError> {
    let state = app.state::<AppState>();
    let call = remote_window(&window, &state)?;
    call.finish(
        &state,
        state
            .client
            .remote_inputs(
                &id,
                &assemble_protocol::remote::RemoteBatch { events },
                &crate::remote::options(&call.cancel),
            )
            .await,
    )
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    /// 稳定错误码：本机自己定的小写加下划线，或者服务端给的大写加下划线。
    pub code: String,
    /// 仅供诊断；界面不显示这段文字。
    pub message: String,
}

impl CommandError {
    pub(crate) fn local(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

impl From<assemble_download::DownloadError> for CommandError {
    fn from(error: assemble_download::DownloadError) -> Self {
        Self::local(error.code(), error.to_string())
    }
}

#[tauri::command]
pub async fn download_prepare(
    app: AppHandle,
    input: assemble_protocol::downloads::DownloadInput,
) -> Result<assemble_protocol::downloads::DownloadPreview, CommandError> {
    app.state::<std::sync::Arc<assemble_download::DownloadManager>>()
        .prepare(input)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn download_import_hls(
    app: AppHandle,
    base_url: Option<String>,
    page_url: Option<String>,
) -> Result<Option<assemble_protocol::downloads::DownloadPreview>, CommandError> {
    let (chosen, answer) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("M3U8", &["m3u8"])
        .pick_file(move |file| {
            let _ = chosen.send(file.and_then(|f| f.as_path().map(std::path::Path::to_path_buf)));
        });
    let Some(path) = answer.await.ok().flatten() else {
        return Ok(None);
    };
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| assemble_download::DownloadError::Storage)?;
    let mut bytes = Vec::new();
    file.take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| assemble_download::DownloadError::Storage)?;
    app.state::<std::sync::Arc<assemble_download::DownloadManager>>()
        .import_hls(bytes, base_url, page_url)
        .await
        .map(Some)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn download_start(
    app: AppHandle,
    preview_id: String,
    variant_id: String,
) -> Result<String, CommandError> {
    app.state::<std::sync::Arc<assemble_download::DownloadManager>>()
        .start(&preview_id, &variant_id)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub fn download_cancel(app: AppHandle, task_id: String) -> Result<(), CommandError> {
    app.state::<std::sync::Arc<assemble_download::DownloadManager>>()
        .cancel(&task_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn download_retry(app: AppHandle, task_id: String) -> Result<(), CommandError> {
    app.state::<std::sync::Arc<assemble_download::DownloadManager>>()
        .retry(&task_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn download_set_assistant_allowed(
    app: AppHandle,
    enabled: bool,
) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let mut settings = state.settings();
    settings.assistant_downloads_allowed = Some(enabled);
    crate::transfers::save_settings(&app, &settings).map_err(|_| {
        CommandError::local(
            "SETTINGS_SAVE_FAILED",
            "download preference could not be saved",
        )
    })?;
    state.edit_settings(|current| current.assistant_downloads_allowed = Some(enabled));
    app.state::<std::sync::Arc<assemble_download::DownloadManager>>()
        .set_assistant_allowed(enabled);
    Ok(state.snapshot())
}

#[tauri::command]
pub async fn download_choose_folder(app: AppHandle) -> Result<(), CommandError> {
    let (chosen, answer) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |folder| {
        let _ = chosen.send(folder.and_then(|f| f.as_path().map(std::path::Path::to_path_buf)));
    });
    if let Some(folder) = answer.await.ok().flatten() {
        app.state::<std::sync::Arc<assemble_download::DownloadManager>>()
            .set_folder(folder)
            .await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn download_open(app: AppHandle, task_id: String) -> Result<(), CommandError> {
    use tauri_plugin_opener::OpenerExt;
    let path = app
        .state::<std::sync::Arc<assemble_download::DownloadManager>>()
        .completed_path(&task_id)
        .await?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|_| CommandError::from(assemble_download::DownloadError::Storage))
}

#[tauri::command]
pub async fn download_reveal(app: AppHandle, task_id: String) -> Result<(), CommandError> {
    let path = app
        .state::<std::sync::Arc<assemble_download::DownloadManager>>()
        .completed_path(&task_id)
        .await?;
    crate::notify::reveal(&app, path);
    Ok(())
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
/// 现在就查一次更新；有新版的话等它下载完才返回。不自动更新的构建原样回它的状态。
#[tauri::command]
pub async fn update_check(app: AppHandle) -> Result<crate::state::UpdateView, CommandError> {
    crate::update::cycle(&app).await;
    Ok(app.state::<AppState>().update_view())
}

/// 换上已经下载好的新版并重新打开窗口。成了就不会回来。
#[tauri::command]
pub fn update_restart(app: AppHandle) -> Result<(), CommandError> {
    crate::update::apply(&app, false)
}

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
    let mut settings = state.settings();
    settings.receive_directory = Some(validated);
    if let Err(error) = crate::transfers::save_settings(&app, &settings) {
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
    let mut settings = state.settings();
    settings.receive_directory = None;
    if let Err(error) = crate::transfers::save_settings(&app, &settings) {
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

/// 挑要发送的文件，然后打开发送面板。文件的位置由系统选择器直接交给 Rust，不经过界面。
#[tauri::command]
pub async fn start_send(app: AppHandle) -> Result<crate::state::Snapshot, CommandError> {
    match crate::send_panel::start(&app).await {
        Ok(()) => Ok(app.state::<AppState>().snapshot()),
        Err(error) => Err(start_failed(&error)),
    }
}

/// 挑文件没成：把原因作为稳定错误码交给界面去显示，技术细节只进日志。
fn start_failed(error: &crate::send_panel::StartError) -> CommandError {
    use crate::send_panel::StartError;
    match error {
        StartError::TooLarge(_) => {
            CommandError::local("FILE_TOO_LARGE", "a chosen file is too large")
        }
        StartError::Unreadable(_) => {
            CommandError::local("FILE_UNREADABLE", "a chosen file could not be read")
        }
        StartError::OnlyFolders => {
            CommandError::local("ONLY_FOLDERS", "only folders were handed over")
        }
        StartError::Window(cause) => {
            tracing::warn!(%cause, "the send panel could not be opened");
            CommandError::local("PANEL_UNAVAILABLE", cause.to_string())
        }
    }
}

/// 把挑好的文件发给一台设备。返回每一条传输的标识，面板据此盯着进度。
#[tauri::command]
pub fn send_to_device(app: AppHandle, device_id: String) -> Result<Vec<String>, CommandError> {
    let state = app.state::<AppState>();
    if state.device_id().as_deref() == Some(device_id.as_str()) {
        return Err(CommandError::local(
            "IS_THIS_DEVICE",
            "a device cannot send files to itself",
        ));
    }
    let Some(device) = state.device(&device_id) else {
        return Err(CommandError::local(
            "NOT_FOUND",
            "this device is not in the account",
        ));
    };
    // 只能发给在线设备。面板里离线的本来选不了；这是它刚好下线时的兜底。
    if !device.online {
        return Err(CommandError::local(
            "DEVICE_OFFLINE",
            "the target device is offline",
        ));
    }
    let ids = state.queue_draft(Destination::Device(device));
    if ids.is_empty() {
        return Err(CommandError::local(
            "NO_DRAFT",
            "there are no files waiting to be sent",
        ));
    }
    Ok(ids)
}

/// 把挑好的文件存进云盘。返回每一条传输的标识。
#[tauri::command]
pub fn send_to_drive(app: AppHandle) -> Result<Vec<String>, CommandError> {
    let state = app.state::<AppState>();
    let ids = state.queue_draft(Destination::Drive);
    if ids.is_empty() {
        return Err(CommandError::local(
            "NO_DRAFT",
            "there are no files waiting to be sent",
        ));
    }
    Ok(ids)
}

/// 向服务端要一份最新的云盘名单。
#[tauri::command]
pub async fn drive_refresh(app: AppHandle) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let listing = state
        .client
        .drive(&RequestOptions::new(&state.lifetime))
        .await
        .map_err(|error| {
            if error.is_unauthorized() {
                session::revoked(&state);
            }
            CommandError::from(&error)
        })?;
    state.set_drive(Some(listing));
    Ok(state.snapshot())
}

/// 挑几个文件存进云盘。位置由系统选择器直接交给 Rust，不经过界面。
#[tauri::command]
pub async fn drive_upload(app: AppHandle) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let files = crate::send_panel::pick(&app, "往云盘里存文件")
        .await
        .map_err(|error| start_failed(&error))?;
    if !files.is_empty() {
        state.queue_files(Destination::Drive, files);
    }
    Ok(state.snapshot())
}

/// 把云盘里的一个文件取回到这台电脑的接收位置。
#[tauri::command]
pub fn drive_download(
    app: AppHandle,
    file_id: String,
) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let Some(file) = state.drive_file(&file_id) else {
        return Err(CommandError::local(
            "FILE_GONE",
            "this file is not in the drive listing",
        ));
    };
    state.fetch_from_drive(&file);
    Ok(state.snapshot())
}

/// 给云盘里的文件改名。只接收服务端标识与名字，不操作本机路径。
#[tauri::command]
pub async fn drive_rename(
    app: AppHandle,
    file_id: String,
    name: String,
) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let name = assemble_protocol::drive::clean_file_name(&name)
        .ok_or_else(|| CommandError::local("INVALID_REQUEST", "invalid drive file name"))?;
    let call = MediaCall::new(&state);
    call.finish(
        &state,
        state
            .client
            .drive_rename(
                &file_id,
                &assemble_protocol::drive::RenameInput {
                    name: name.to_owned(),
                },
                &RequestOptions::new(&call.cancel),
            )
            .await,
    )?;
    // 服务端名单包含其他设备的改动；读取失败时让用户保留草稿，重试改名仍然幂等。
    let listing = call.finish(
        &state,
        state.client.drive(&RequestOptions::new(&call.cancel)).await,
    )?;
    state.set_drive(Some(listing));
    Ok(state.snapshot())
}

/// 预览只对登录的主窗口开放；界面不接触读取凭据或本机路径。
#[tauri::command]
pub async fn drive_preview_open(
    app: AppHandle,
    window: tauri::WebviewWindow,
    file_id: String,
    request_id: String,
) -> Result<crate::drive_preview::OpenedPreview, CommandError> {
    let state = app.state::<AppState>();
    remote_window(&window, &state)?;
    crate::drive_preview::open(&state, &file_id, &request_id).await
}

#[tauri::command]
pub fn drive_preview_close(
    app: AppHandle,
    window: tauri::WebviewWindow,
    request_id: String,
) -> Result<(), CommandError> {
    if !assemble_protocol::primitives::is_valid_id(&request_id) {
        return Err(CommandError::local(
            "INVALID_INPUT",
            "invalid preview request identifier",
        ));
    }
    if window.label() != crate::state::MAIN_WINDOW {
        return Err(CommandError::local(
            "FORBIDDEN",
            "preview requires main window",
        ));
    }
    app.state::<AppState>()
        .file_previews
        .close_request(&request_id);
    Ok(())
}

/// 从云盘里删掉一个文件。
#[tauri::command]
pub async fn drive_delete(
    app: AppHandle,
    file_id: String,
) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let deleted = state
        .client
        .drive_delete(&file_id, &RequestOptions::new(&state.lifetime))
        .await;
    match deleted {
        Ok(()) => {}
        Err(error) => {
            if error.is_unauthorized() {
                session::revoked(&state);
            }
            tracing::debug!(%error, "a drive file could not be removed");
            return Err(CommandError::from(&error));
        }
    }
    // 以服务端的名单为准：别处的改动也一并带回来。
    match state
        .client
        .drive(&RequestOptions::new(&state.lifetime))
        .await
    {
        Ok(listing) => state.set_drive(Some(listing)),
        Err(error) => tracing::debug!(%error, "the drive could not be listed after a removal"),
    }
    Ok(state.snapshot())
}

/// 关掉发送面板。挑好却没发的文件不再留着。
#[tauri::command]
pub fn close_send_panel(app: AppHandle) {
    crate::send_panel::close(&app);
}

/// 把主窗口叫到前面。发送面板里"请先登录"时用。
#[tauri::command]
pub fn open_main(app: AppHandle) {
    crate::state::show_main(&app);
}

/// 只能打开当前服务的固定影视站，界面不能传入任意网址。
#[tauri::command]
pub fn open_public_media(app: AppHandle) -> Result<(), CommandError> {
    use tauri_plugin_opener::OpenerExt;
    let state = app.state::<AppState>();
    let address = state.client.origin().join("/movies/");
    app.opener()
        .open_url(address.as_str(), None::<&str>)
        .map_err(|_| CommandError::local("BROWSER_UNAVAILABLE", "browser could not be opened"))
}

/// 地点由服务端按当前身份重新读出，界面只交记录标识和显示名称。
#[tauri::command]
pub async fn open_record_location(
    app: AppHandle,
    resource: String,
    id: String,
    label: String,
) -> Result<(), CommandError> {
    use tauri_plugin_opener::OpenerExt;
    let state = app.state::<AppState>();
    let locations = state
        .client
        .record_locations(&resource, &id, &RequestOptions::new(&state.lifetime))
        .await
        .map_err(|error| planner_error(&error))?;
    let url = locations
        .iter()
        .find(|location| location.label == label)
        .and_then(assemble_protocol::schedule::Location::directions_url)
        .ok_or_else(|| CommandError::local("NOT_FOUND", "the location is no longer available"))?;
    app.opener()
        .open_url(url.as_str(), None::<&str>)
        .map_err(|_| CommandError::local("BROWSER_UNAVAILABLE", "maps could not be opened"))
}

/// 取消一条传输：还没发完的不发了，已经发到服务端的撤回。
#[tauri::command]
pub async fn cancel_transfer(app: AppHandle, transfer_id: String) {
    crate::transfers::cancel(&app.state::<AppState>(), &transfer_id).await;
}

/// 再发一次。只对"文件根本没到服务端"的失败有用。
#[tauri::command]
pub fn retry_transfer(app: AppHandle, transfer_id: String) -> Result<(), CommandError> {
    if crate::transfers::retry_send(&app.state::<AppState>(), &transfer_id) {
        Ok(())
    } else {
        Err(CommandError::local(
            "NOT_RETRYABLE",
            "only a send that never reached the server can be retried",
        ))
    }
}

/// 在文件的右键菜单里加入（或去掉）"用 Assemble 发送"。
/// 只有 Windows 上的打包运行支持；别的地方调用会得到 `UNSUPPORTED`。
#[tauri::command]
pub fn set_send_menu(
    app: AppHandle,
    enabled: bool,
) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    if !crate::send_menu::supported() {
        return Err(CommandError::local(
            "UNSUPPORTED",
            "the send menu is only available in packaged builds on Windows",
        ));
    }
    let mut settings = state.settings();
    if let Err(error) = crate::send_menu::set(&mut settings, enabled) {
        tracing::warn!(%error, "the send menu could not be changed");
        return Err(CommandError::local(
            "SYSTEM_REJECTED",
            "the send menu could not be changed",
        ));
    }
    if let Err(error) = crate::transfers::save_settings(&app, &settings) {
        // 注册表已经改了，只是这个选择没能记下来：下次启动会按默认值再登记一遍。
        tracing::warn!(%error, "the send menu choice could not be saved");
    }
    state.set_send_menu(enabled);
    Ok(state.snapshot())
}

// ---- 日程规划 ----

/// 界面每一次提交新建时带的新幂等键：同一毫秒里的两次提交靠序号分开。
static SUBMISSIONS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// 日程与提醒的内容不合格。诊断沿用原来的那一句，界面不显示它。
fn rejected(error: &assemble_client::ClientError) -> CommandError {
    CommandError::local("PLANNER_INPUT", error.to_string())
}

fn submission_key() -> String {
    use std::sync::atomic::Ordering;

    let now = assemble_protocol::primitives::Timestamp::now().as_millis();
    let sequence = SUBMISSIONS.fetch_add(1, Ordering::Relaxed);
    format!("ui-{now}-{sequence}")
}

/// 改动之后以服务端的名单为准。事件流会推一份过来，但万一它正断着，这里再拉一次。
async fn settle_planner(state: &AppState) {
    if let Err(error) = crate::planner::refresh(state, &state.lifetime).await {
        tracing::debug!(%error, "the planner could not be listed after a change");
    }
}

/// 日程与提醒的内容不合格时给界面的错误码。
///
/// 内容先在客户端按服务端同一套规则校验，不合格的还没发出去就拦下了；服务端万一再拒一次，
/// 用的也是 `INVALID_REQUEST`。两种情形说的是同一件事，界面给同一句话——
/// 不是设备改名那套说辞。
fn planner_error(error: &assemble_client::ClientError) -> CommandError {
    use assemble_client::ClientError;

    match error {
        ClientError::InvalidRequest => rejected(error),
        ClientError::Http { remote_code, .. }
            if remote_code.as_deref() == Some(assemble_protocol::error::codes::INVALID_REQUEST) =>
        {
            rejected(error)
        }
        other => CommandError::from(other),
    }
}

async fn planner_change<T>(
    state: &AppState,
    action: impl std::future::Future<Output = Result<T, assemble_client::ClientError>>,
) -> Result<crate::state::Snapshot, CommandError> {
    action.await.map_err(|error| planner_error(&error))?;
    settle_planner(state).await;
    Ok(state.snapshot())
}

/// 拉一次日程名单。
#[tauri::command]
pub async fn planner_refresh(app: AppHandle) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    match crate::planner::refresh(&state, &state.lifetime).await {
        Ok(()) => Ok(state.snapshot()),
        Err(error) => Err(CommandError::from(&error)),
    }
}

/// 新建一条日程。带一个新的幂等键：请求超时之后重试，不会建出两条。
#[tauri::command]
pub async fn schedule_create(
    app: AppHandle,
    content: assemble_protocol::schedule::ScheduleContent,
) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let options = RequestOptions::new(&state.lifetime);
    let key = submission_key();
    let change = state.client.create_schedule(&content, Some(&key), &options);
    planner_change(&state, change).await
}

/// 整份替换一条日程的内容。带上看到的版本号；别处改过时得到 `VERSION_CONFLICT`。
#[tauri::command]
pub async fn schedule_update(
    app: AppHandle,
    id: String,
    version: i64,
    content: assemble_protocol::schedule::ScheduleContent,
) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let options = RequestOptions::new(&state.lifetime);
    let update = assemble_protocol::schedule::ScheduleUpdate { version, content };
    let change = state.client.update_schedule(&id, &update, &options);
    planner_change(&state, change).await
}

macro_rules! schedule_action {
    ($name:ident, $action:expr, $doc:literal) => {
        #[doc = $doc]
        #[tauri::command]
        pub async fn $name(
            app: AppHandle,
            id: String,
            version: i64,
        ) -> Result<crate::state::Snapshot, CommandError> {
            let state = app.state::<AppState>();
            let options = RequestOptions::new(&state.lifetime);
            let change = state
                .client
                .set_schedule_status(&id, $action, version, &options);
            planner_change(&state, change).await
        }
    };
}

schedule_action!(
    schedule_complete,
    assemble_client::ScheduleAction::Complete,
    "整条日程标成已完成。"
);
schedule_action!(
    schedule_cancel,
    assemble_client::ScheduleAction::Cancel,
    "取消一条日程：还没读的提醒随之作废。"
);
schedule_action!(
    schedule_reopen,
    assemble_client::ScheduleAction::Reopen,
    "完成或取消之后改回进行中。"
);

/// 删掉一条日程，连它的通知一起。
#[tauri::command]
pub async fn schedule_delete(
    app: AppHandle,
    id: String,
    version: i64,
) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let options = RequestOptions::new(&state.lifetime);
    let change = state.client.delete_schedule(&id, version, &options);
    planner_change(&state, change).await
}

/// 新建一条提醒。幂等键的用处同日程。
#[tauri::command]
pub async fn reminder_create(
    app: AppHandle,
    content: assemble_protocol::schedule::ReminderContent,
) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let options = RequestOptions::new(&state.lifetime);
    let key = submission_key();
    let change = state.client.create_reminder(&content, Some(&key), &options);
    planner_change(&state, change).await
}

/// 整份替换一条提醒的内容。已经到点的不能改，得到 `INVALID_STATE`。
#[tauri::command]
pub async fn reminder_update(
    app: AppHandle,
    id: String,
    version: i64,
    content: assemble_protocol::schedule::ReminderContent,
) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let options = RequestOptions::new(&state.lifetime);
    let update = assemble_protocol::schedule::ReminderUpdate { version, content };
    let change = state.client.update_reminder(&id, &update, &options);
    planner_change(&state, change).await
}

macro_rules! reminder_action {
    ($name:ident, $action:expr, $doc:literal) => {
        #[doc = $doc]
        #[tauri::command]
        pub async fn $name(
            app: AppHandle,
            id: String,
            version: i64,
        ) -> Result<crate::state::Snapshot, CommandError> {
            let state = app.state::<AppState>();
            let options = RequestOptions::new(&state.lifetime);
            let change = state
                .client
                .set_reminder_status(&id, $action, version, &options);
            planner_change(&state, change).await
        }
    };
}

reminder_action!(
    reminder_dismiss,
    assemble_client::ReminderAction::Dismiss,
    "一条提醒办完了，或者到点之后看过了。"
);
reminder_action!(
    reminder_cancel,
    assemble_client::ReminderAction::Cancel,
    "不提醒了。"
);

/// 删掉一条提醒，连它的通知一起。
#[tauri::command]
pub async fn reminder_delete(
    app: AppHandle,
    id: String,
    version: i64,
) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let options = RequestOptions::new(&state.lifetime);
    let change = state.client.delete_reminder(&id, version, &options);
    planner_change(&state, change).await
}

/// 把几条通知标成已读。
#[tauri::command]
pub async fn notifications_read(
    app: AppHandle,
    ids: Vec<String>,
) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let options = RequestOptions::new(&state.lifetime);
    let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
    let change = state.client.mark_notifications_read(&borrowed, &options);
    planner_change(&state, change).await
}

/// 这个账户的通知全部标成已读。
#[tauri::command]
pub async fn notifications_read_all(
    app: AppHandle,
) -> Result<crate::state::Snapshot, CommandError> {
    let state = app.state::<AppState>();
    let options = RequestOptions::new(&state.lifetime);
    let change = state.client.mark_all_notifications_read(&options);
    planner_change(&state, change).await
}

/// 点开了一条通知（或者看完收起）：主窗口定位到它。传 `None` 取消定位。
#[tauri::command]
pub fn planner_focus(app: AppHandle, notification_id: Option<String>) -> crate::state::Snapshot {
    let state = app.state::<AppState>();
    state.set_planner_focus(notification_id);
    state.snapshot()
}

/// 摆出一张新建表单（托盘的"新建提醒…"用），或者把它收起。
#[tauri::command]
pub fn planner_compose(
    app: AppHandle,
    compose: Option<crate::planner::Compose>,
) -> crate::state::Snapshot {
    let state = app.state::<AppState>();
    state.set_planner_compose(compose);
    state.snapshot()
}

/// 这台电脑到点时要不要弹提醒。
#[tauri::command]
pub fn set_planner_muted(app: AppHandle, muted: bool) -> crate::state::Snapshot {
    let state = app.state::<AppState>();
    state.set_planner_muted(muted);
    if let Err(error) = crate::transfers::save_settings(&app, &state.settings()) {
        // 这次已经按新设置走了，只是没记下来：下次启动回到原来的选择。
        tracing::warn!(%error, "the reminder choice could not be saved");
    }
    state.snapshot()
}

/// 提示看过了，收起来。
#[tauri::command]
pub fn dismiss_notice(app: AppHandle) -> crate::state::Snapshot {
    let state = app.state::<AppState>();
    state.dismiss_notice();
    state.snapshot()
}

#[tauri::command]
pub fn system_notifications_refresh(app: AppHandle) -> crate::state::Snapshot {
    app.state::<AppState>().refresh_system_notifications()
}

#[tauri::command]
pub fn system_notifications_read(
    app: AppHandle,
    items: Vec<assemble_protocol::system_notifications::SystemNotificationRead>,
) -> Result<crate::state::Snapshot, CommandError> {
    app.state::<AppState>()
        .read_system_notifications(&items)
        .map_err(|()| {
            CommandError::local(
                "notification_store_unavailable",
                "system notifications could not be saved",
            )
        })
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

// 私人查询按会话分界：退出或切换账户后，旧请求不能把内容交给新页面。
struct MediaCall {
    epoch: u64,
    cancel: tokio_util::sync::CancellationToken,
}

impl MediaCall {
    fn new<R: tauri::Runtime>(state: &AppState<R>) -> Self {
        Self {
            epoch: *state.session_changes().borrow(),
            cancel: if state.device_id().is_some() {
                state.session_cancel()
            } else {
                state.lifetime.child_token()
            },
        }
    }

    fn finish<T, R: tauri::Runtime>(
        &self,
        state: &AppState<R>,
        result: Result<T, assemble_client::ClientError>,
    ) -> Result<T, CommandError> {
        if self.cancel.is_cancelled() || *state.session_changes().borrow() != self.epoch {
            return Err(CommandError::local(
                "CANCELLED",
                "the account session changed",
            ));
        }
        result.map_err(|error| {
            if error.is_unauthorized() {
                session::revoked(state);
            }
            CommandError::from(&error)
        })
    }
}

use assemble_protocol::media::{
    MediaActorCredits, MediaCandidate, MediaCapabilities, MediaCreate, MediaEntry, MediaGallery,
    MediaIdentity, MediaImport, MediaJob, MediaList, MediaPersonIdentity, MediaPoster,
    MediaProvider, MediaQuery, MediaScope, MediaSync, MediaType, MediaUpdate,
};

fn memo_call<R: tauri::Runtime>(state: &AppState<R>) -> Result<MediaCall, CommandError> {
    if state.device_id().is_none() {
        return Err(CommandError::local(
            "UNAUTHORIZED",
            "memo access requires an account",
        ));
    }
    Ok(MediaCall::new(state))
}

#[tauri::command]
pub async fn memo_list(app: AppHandle, query: MemoQuery) -> Result<MemoList, CommandError> {
    let state = app.state::<AppState>();
    let call = memo_call(&state)?;
    call.finish(
        &state,
        state
            .client
            .memo_list(&query, &RequestOptions::new(&call.cancel))
            .await,
    )
}
#[tauri::command]
pub async fn memo_get(app: AppHandle, id: String) -> Result<Memo, CommandError> {
    let state = app.state::<AppState>();
    let call = memo_call(&state)?;
    call.finish(
        &state,
        state
            .client
            .memo_get(&id, &RequestOptions::new(&call.cancel))
            .await,
    )
}
#[tauri::command]
pub async fn memo_create(app: AppHandle, input: MemoCreate) -> Result<Memo, CommandError> {
    let state = app.state::<AppState>();
    let call = memo_call(&state)?;
    call.finish(
        &state,
        state
            .client
            .memo_create(&input, &RequestOptions::new(&call.cancel))
            .await,
    )
}
#[tauri::command]
pub async fn memo_update(
    app: AppHandle,
    id: String,
    input: MemoUpdate,
) -> Result<Memo, CommandError> {
    let state = app.state::<AppState>();
    let call = memo_call(&state)?;
    call.finish(
        &state,
        state
            .client
            .memo_update(&id, &input, &RequestOptions::new(&call.cancel))
            .await,
    )
}
#[tauri::command]
pub async fn memo_remove(app: AppHandle, id: String, version: i64) -> Result<(), CommandError> {
    let state = app.state::<AppState>();
    let call = memo_call(&state)?;
    call.finish(
        &state,
        state
            .client
            .memo_remove(&id, version, &RequestOptions::new(&call.cancel))
            .await,
    )
}
#[tauri::command]
pub async fn memo_copy(app: AppHandle, id: String) -> Result<(), CommandError> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let state = app.state::<AppState>();
    let call = memo_call(&state)?;
    let memo = call.finish(
        &state,
        state
            .client
            .memo_get(&id, &RequestOptions::new(&call.cancel))
            .await,
    )?;
    app.clipboard()
        .write_text(memo.content.text)
        .map_err(|_| CommandError::local("SYSTEM_REJECTED", "the clipboard could not be written"))
}

#[tauri::command]
pub async fn media_capabilities(app: AppHandle) -> Result<MediaCapabilities, CommandError> {
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    call.finish(
        &state,
        state
            .client
            .media_capabilities(&RequestOptions::new(&call.cancel))
            .await,
    )
}
#[tauri::command]
pub async fn media_list(
    app: AppHandle,
    scope: MediaScope,
    query: MediaQuery,
) -> Result<MediaList, CommandError> {
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    call.finish(
        &state,
        state
            .client
            .media_list(scope, &query, &RequestOptions::new(&call.cancel))
            .await,
    )
}
#[tauri::command]
pub async fn media_get(
    app: AppHandle,
    scope: MediaScope,
    id: String,
) -> Result<MediaEntry, CommandError> {
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    call.finish(
        &state,
        state
            .client
            .media_get(scope, &id, &RequestOptions::new(&call.cancel))
            .await,
    )
}
// 桌面只维护自己的片库；即便界面参数被篡改，也没有共享维护入口。
#[tauri::command]
pub async fn media_create(app: AppHandle, input: MediaCreate) -> Result<MediaEntry, CommandError> {
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    call.finish(
        &state,
        state
            .client
            .media_create(
                MediaScope::Private,
                &input,
                &RequestOptions::new(&call.cancel),
            )
            .await,
    )
}
#[tauri::command]
pub async fn media_update(
    app: AppHandle,
    id: String,
    input: MediaUpdate,
) -> Result<MediaEntry, CommandError> {
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    call.finish(
        &state,
        state
            .client
            .media_update(
                MediaScope::Private,
                &id,
                &input,
                &RequestOptions::new(&call.cancel),
            )
            .await,
    )
}
#[tauri::command]
pub async fn media_remove(app: AppHandle, id: String, version: u64) -> Result<(), CommandError> {
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    call.finish(
        &state,
        state
            .client
            .media_remove(
                MediaScope::Private,
                &id,
                version,
                &RequestOptions::new(&call.cancel),
            )
            .await,
    )
}
#[tauri::command]
pub async fn media_search(
    app: AppHandle,
    provider: MediaProvider,
    media_type: MediaType,
    q: String,
) -> Result<Vec<MediaCandidate>, CommandError> {
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    call.finish(
        &state,
        state
            .client
            .media_search(provider, media_type, &q, &RequestOptions::new(&call.cancel))
            .await,
    )
}
#[tauri::command]
pub async fn media_catalog(
    app: AppHandle,
    identity: MediaIdentity,
) -> Result<MediaCandidate, CommandError> {
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    call.finish(
        &state,
        state
            .client
            .media_catalog(&identity, &RequestOptions::new(&call.cancel))
            .await,
    )
}
#[tauri::command]
pub async fn media_actor(
    app: AppHandle,
    identity: MediaPersonIdentity,
    media_type: MediaType,
    page: Option<u32>,
    page_size: Option<u32>,
) -> Result<MediaActorCredits, CommandError> {
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    call.finish(
        &state,
        state
            .client
            .media_actor(
                &identity,
                media_type,
                page.unwrap_or(1),
                page_size.unwrap_or(24),
                &RequestOptions::new(&call.cancel),
            )
            .await,
    )
}
#[tauri::command]
pub async fn media_import(app: AppHandle, input: MediaImport) -> Result<MediaJob, CommandError> {
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    call.finish(
        &state,
        state
            .client
            .media_import(
                MediaScope::Private,
                &input,
                &RequestOptions::new(&call.cancel),
            )
            .await,
    )
}
#[tauri::command]
pub async fn media_job(app: AppHandle, id: String) -> Result<MediaJob, CommandError> {
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    call.finish(
        &state,
        state
            .client
            .media_job(MediaScope::Private, &id, &RequestOptions::new(&call.cancel))
            .await,
    )
}
#[tauri::command]
pub async fn media_gallery(
    app: AppHandle,
    scope: MediaScope,
    id: String,
) -> Result<MediaGallery, CommandError> {
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    call.finish(
        &state,
        state
            .client
            .media_gallery(scope, &id, &RequestOptions::new(&call.cancel))
            .await,
    )
}
#[tauri::command]
pub async fn media_sync_gallery(
    app: AppHandle,
    id: String,
    input: MediaSync,
) -> Result<MediaJob, CommandError> {
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    call.finish(
        &state,
        state
            .client
            .media_sync_gallery(
                MediaScope::Private,
                &id,
                &input,
                &RequestOptions::new(&call.cancel),
            )
            .await,
    )
}
#[tauri::command]
pub async fn media_select_poster(
    app: AppHandle,
    id: String,
    input: MediaPoster,
) -> Result<MediaEntry, CommandError> {
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    call.finish(
        &state,
        state
            .client
            .media_select_poster(
                MediaScope::Private,
                &id,
                &input,
                &RequestOptions::new(&call.cancel),
            )
            .await,
    )
}
#[tauri::command]
pub async fn media_image(
    app: AppHandle,
    asset_id: String,
    variant: String,
) -> Result<String, CommandError> {
    use base64::Engine;
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    let asset = call.finish(
        &state,
        state
            .client
            .media_asset(&asset_id, &variant, &RequestOptions::new(&call.cancel))
            .await,
    )?;
    if !matches!(
        asset.content_type.as_str(),
        "image/jpeg" | "image/png" | "image/webp"
    ) {
        return Err(CommandError::local(
            "INVALID_STATE",
            "the media asset is not an image",
        ));
    }
    Ok(format!(
        "data:{};base64,{}",
        asset.content_type,
        base64::engine::general_purpose::STANDARD.encode(asset.bytes)
    ))
}
#[tauri::command]
pub async fn media_copy_magnet(
    app: AppHandle,
    scope: MediaScope,
    id: String,
) -> Result<(), CommandError> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let state = app.state::<AppState>();
    let call = MediaCall::new(&state);
    let entry = call.finish(
        &state,
        state
            .client
            .media_get(scope, &id, &RequestOptions::new(&call.cancel))
            .await,
    )?;
    let magnet = entry
        .content
        .magnet_uri
        .ok_or_else(|| CommandError::local("NOT_FOUND", "no permitted magnet link"))?;
    app.clipboard()
        .write_text(magnet)
        .map_err(|_| CommandError::local("SYSTEM_REJECTED", "the clipboard could not be written"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SilentNotifications;
    impl crate::state::Notifier for SilentNotifications {
        fn received(&self, _: i32, _: &str, _: &str, _: bool) {}
        fn reminder(&self, _: i32, _: &str, _: &str) {}
    }

    #[tokio::test]
    async fn stale_media_responses_cannot_cross_a_session_change() {
        let app = tauri::test::mock_app();
        let client =
            assemble_client::Client::new(assemble_client::ServerOrigin::development()).unwrap();
        app.manage(AppState::new(
            app.handle().clone(),
            client,
            None,
            crate::state::Settings::default(),
            std::path::PathBuf::from("Downloads"),
            std::sync::Arc::new(SilentNotifications),
        ));
        let state = app.state::<AppState<tauri::test::MockRuntime>>();
        assert_eq!(memo_call(&state).err().unwrap().code, "UNAUTHORIZED");
        let old = MediaCall::new(&state);
        assert_eq!(old.finish(&state, Ok(42)).unwrap(), 42);
        state.begin_session();
        assert_eq!(old.finish(&state, Ok(43)).unwrap_err().code, "CANCELLED");
        let before = *state.session_changes().borrow();
        let stale_unauthorized = Err::<(), _>(assemble_client::ClientError::Http {
            status: 401,
            remote_code: Some("UNAUTHORIZED".into()),
        });
        assert_eq!(
            old.finish(&state, stale_unauthorized).unwrap_err().code,
            "CANCELLED"
        );
        // 旧请求的401也不许撤销刚开始的新会话。
        assert_eq!(*state.session_changes().borrow(), before);
        let current = MediaCall::new(&state);
        current.cancel.cancel();
        assert_eq!(
            current.finish(&state, Ok(44)).unwrap_err().code,
            "CANCELLED"
        );
    }

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
    fn planner_content_that_is_not_acceptable_gets_its_own_code() {
        use assemble_client::ClientError;

        // 客户端自己拦下的，与服务端用 `INVALID_REQUEST` 再拒一次的，是同一句话。
        assert_eq!(
            planner_error(&ClientError::InvalidRequest).code,
            "PLANNER_INPUT"
        );
        let refused = ClientError::Http {
            status: 400,
            remote_code: Some("INVALID_REQUEST".to_owned()),
        };
        assert_eq!(planner_error(&refused).code, "PLANNER_INPUT");
        // 别的错误照旧：版本冲突要原样交给界面。
        let conflict = ClientError::Http {
            status: 409,
            remote_code: Some("VERSION_CONFLICT".to_owned()),
        };
        assert_eq!(planner_error(&conflict).code, "VERSION_CONFLICT");
        assert_eq!(planner_error(&ClientError::Network).code, "UNREACHABLE");
    }

    #[test]
    fn server_codes_are_uppercase_words() {
        assert!(is_server_code("QUOTA_EXCEEDED"));
        assert!(!is_server_code(""));
        assert!(!is_server_code("lowercase"));
        assert!(!is_server_code("WITH SPACE"));
    }
}
