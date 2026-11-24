//! 发送面板：用户挑好文件，再选一台设备（或者云盘）发过去。
//!
//! 文件从三个地方来，位置都直接交给 Rust，不经过界面：
//!
//! - 菜单栏或"传输"卡片里的"发送文件…"：系统的文件选择框；
//! - Windows 的右键菜单"用 Assemble 发送"：文件路径作为启动参数交给应用
//!   （见 `assemble_device::send_menu`）；
//! - 把文件拖进主窗口。
//!
//! 面板里的界面只看得到文件的名字与大小，以及账户里的设备；它能调用的命令只认标识，
//! 不认路径。面板是一个单独的窗口，能力清单里只有"接收状态"这一项。

use std::path::PathBuf;

use assemble_client::{ClientError, OutgoingFile};
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;

use crate::state::{AppState, MAIN_WINDOW, Notice, show_main};

/// 发送面板窗口的标签。
pub const SEND_WINDOW: &str = "send";

/// 没法开始发送的原因。带的是文件名，不是路径：提示里要说得出是哪个文件。
#[derive(Debug)]
pub enum StartError {
    TooLarge(String),
    Unreadable(String),
    /// 拖进来或选中的只有文件夹。
    OnlyFolders,
    Window(tauri::Error),
}

impl StartError {
    /// 给用户看的一句话。
    fn notice(&self) -> Notice {
        match self {
            Self::TooLarge(name) => Notice::error(format!("「{name}」超过 100 MB，发不出去。")),
            Self::Unreadable(name) => {
                Notice::error(format!("「{name}」读不出来，可能已被移走或没有权限。"))
            }
            Self::OnlyFolders => Notice::error("只能发送文件，不能发送文件夹。"),
            Self::Window(_) => Notice::error("发送面板没能打开，请重试。"),
        }
    }
}

/// 让用户挑要发送的文件。关掉选择框时得到空列表。
pub async fn pick_files<R: Runtime>(app: &AppHandle<R>) -> Vec<PathBuf> {
    let (chosen, answer) = tokio::sync::oneshot::channel();
    let mut picker = app.dialog().file().set_title("选择要发送的文件");
    // 选择框挂在面板上；面板还没打开时挂在主窗口上。
    let parent = app
        .get_webview_window(SEND_WINDOW)
        .or_else(|| app.get_webview_window(MAIN_WINDOW));
    if let Some(window) = parent {
        picker = picker.set_parent(&window);
    }
    picker.pick_files(move |files| {
        // 选中的若是一个网址而不是本机文件，不算数。
        let paths = files
            .unwrap_or_default()
            .into_iter()
            .filter_map(|file| file.as_path().map(std::path::Path::to_path_buf))
            .collect();
        let _ = chosen.send(paths);
    });
    answer.await.unwrap_or_default()
}

/// 让用户挑几个文件，并逐个核对好（大小合规、算出摘要）。
///
/// 没登录时什么都挑不了：把主窗口叫到前面，请用户先登录。`purpose` 用在那句提示里。
/// 关掉选择框、或者没登录时得到空列表。
pub async fn pick<R: Runtime>(
    app: &AppHandle<R>,
    purpose: &str,
) -> Result<Vec<OutgoingFile>, StartError> {
    let state = app.state::<AppState>();
    if state.device_id().is_none() {
        state.set_notice(Some(Notice::info(format!("请先登录，再{purpose}。"))));
        show_main(app);
        return Ok(Vec::new());
    }
    // 什么都没挑：什么都不做。
    let paths = pick_files(app).await;
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    open_files(app, paths).await
}

/// 把交给应用的路径逐个核对好：大小合规、算出摘要。之后面板与上传用的都是这份结果。
///
/// 文件夹不算：拖进来一整摞东西时，其中的文件照发。只有文件夹时如实说发不了。
async fn open_files<R: Runtime>(
    app: &AppHandle<R>,
    paths: Vec<PathBuf>,
) -> Result<Vec<OutgoingFile>, StartError> {
    let cancel = app.state::<AppState>().lifetime.child_token();
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        if path.is_dir() {
            continue;
        }
        let name = file_name(&path);
        match OutgoingFile::open(&path, &cancel).await {
            Ok(file) => files.push(file),
            Err(ClientError::TooLarge) => return Err(StartError::TooLarge(name)),
            Err(_) => return Err(StartError::Unreadable(name)),
        }
    }
    if files.is_empty() {
        return Err(StartError::OnlyFolders);
    }
    Ok(files)
}

/// 用户要发送文件：先挑文件，再打开发送面板。
pub async fn start<R: Runtime>(app: &AppHandle<R>) -> Result<(), StartError> {
    let files = pick(app, "发送文件").await?;
    if files.is_empty() {
        return Ok(());
    }
    app.state::<AppState>().set_draft(files);
    open(app).map_err(StartError::Window)
}

/// 文件已经交到应用手里（右键菜单、拖放）：不挑了，直接打开发送面板。
pub async fn start_with<R: Runtime>(
    app: &AppHandle<R>,
    paths: Vec<PathBuf>,
) -> Result<(), StartError> {
    if paths.is_empty() {
        return Ok(());
    }
    // 没登录时发不了：把主窗口叫到前面，请用户先登录。
    let state = app.state::<AppState>();
    if state.device_id().is_none() {
        state.set_notice(Some(Notice::info("请先登录，再发送文件。")));
        show_main(app);
        return Ok(());
    }
    let files = open_files(app, paths).await?;
    app.state::<AppState>().set_draft(files);
    open(app).map_err(StartError::Window)
}

/// 开始发送没成：把原因告诉用户。
pub fn report<R: Runtime>(app: &AppHandle<R>, error: &StartError) {
    if let StartError::Window(cause) = error {
        tracing::warn!(%cause, "the send panel could not be opened");
    }
    app.state::<AppState>().set_notice(Some(error.notice()));
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || "这个文件".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// 打开（或叫到前面）发送面板。先以隐藏状态创建，加载好再显示。
pub fn open<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if app.get_webview_window(SEND_WINDOW).is_none() {
        WebviewWindowBuilder::new(
            app,
            SEND_WINDOW,
            WebviewUrl::App("index.html?window=send".into()),
        )
        .title("发送到…")
        .inner_size(440.0, 560.0)
        .min_inner_size(380.0, 320.0)
        .center()
        .resizable(true)
        .visible(false)
        .build()?;
    }
    let Some(window) = app.get_webview_window(SEND_WINDOW) else {
        return Ok(());
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    Ok(())
}

/// 关掉发送面板。挑好却没发的文件不再留着。
pub fn close<R: Runtime>(app: &AppHandle<R>) {
    app.state::<AppState>().drop_draft();
    if let Some(window) = app.get_webview_window(SEND_WINDOW) {
        let _ = window.close();
    }
}

/// 发送面板正在关闭：挑好却没发的文件不再留着。
pub fn closed<R: Runtime>(app: &AppHandle<R>) {
    app.state::<AppState>().drop_draft();
}
