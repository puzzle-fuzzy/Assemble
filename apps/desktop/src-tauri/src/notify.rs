//! 系统通知：文件收到之后告诉用户；点一下就在访达或资源管理器里显示它。
//!
//! 通知里只有"谁发来了哪个文件"。文件在本机的位置不出通知：
//! 点通知时由 Rust 按通知的标识找回它。只显示、不打开——收到的东西是不信任的内容，
//! 要不要打开由用户自己决定。
//!
//! 通知发出去之后能不能显示、什么时候消失，都由系统决定，这里不等它。

use std::path::PathBuf;

use tauri::AppHandle;
use tauri::Manager;
use tauri_plugin_notification::{ActionPerformed, ActiveNotification, NotificationExt};
use tauri_plugin_opener::OpenerExt;

use crate::state::{AppState, Notifier};

/// 点通知时系统报上来的动作：点的是通知本身（没有别的按钮可点）。
const TAP: &str = "tap";

/// 登记点击的处理。通知发出去之前就得登记好，早于登记的通知点了也不会报上来。
pub fn install(app: &AppHandle) {
    let watcher = app.clone();
    if let Err(error) = app
        .notification()
        .on_action(move |performed: &ActionPerformed| {
            if performed.action_id() != TAP {
                return;
            }
            let Some(handle) = performed.notification().map(ActiveNotification::id) else {
                return;
            };
            let state = watcher.state::<AppState>();
            if let Some(path) = state.saved_file_for_handle(handle) {
                reveal(&watcher, path);
            }
        })
    {
        tracing::warn!(%error, "notification clicks will not be reported");
    }
}

/// 用系统通知告诉用户。点击的处理在 [`install`] 里登记。
pub struct SystemNotifier {
    app: AppHandle,
}

impl SystemNotifier {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl Notifier for SystemNotifier {
    fn received(&self, handle: i32, peer: &str, file_name: &str, fell_back: bool) {
        received(&self.app, handle, peer, file_name, fell_back);
    }
}

/// 文件收到了。
///
/// `handle` 是这条通知的标识，得先在传输记录里登记好再调用：
/// 点通知时按它找回文件。
pub fn received(app: &AppHandle, handle: i32, peer: &str, file_name: &str, fell_back: bool) {
    let mut body = format!("「{peer}」发来了「{file_name}」。");
    if fell_back {
        body.push_str("原来设置的接收文件夹不可用，已存到默认位置。");
    }
    if let Err(error) = app
        .notification()
        .builder()
        .title("收到文件")
        .body(body)
        .id(handle)
        .show()
    {
        tracing::debug!(%error, "a notification could not be shown");
    }
}

/// 在访达或资源管理器里显示一个文件：只选中它，不打开它。
pub fn reveal<R: tauri::Runtime>(app: &AppHandle<R>, path: PathBuf) {
    let opener = app.clone();
    let shown = app.run_on_main_thread(move || {
        if let Err(error) = opener.opener().reveal_item_in_dir(&path) {
            tracing::debug!(%error, "the received file could not be shown in its folder");
        }
    });
    if let Err(error) = shown {
        tracing::debug!(%error, "the received file could not be shown in its folder");
    }
}
