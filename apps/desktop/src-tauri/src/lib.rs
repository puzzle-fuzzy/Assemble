//! Assemble 桌面壳。
//!
//! 这里只做"壳"的事：窗口、托盘、开机启动、生命周期，以及把 Rust 核心的能力
//! 以少量具名命令的形式交给界面。业务规则都在 `crates/` 里，可以脱离界面测试。
//!
//! 连接哪个服务由构建决定：调试构建连本地开发服务，发布构建连正式服务。
//! 服务地址不从界面输入，也不从用户配置读取。

// Tauri 命令按值接收 `AppHandle`，这是框架的调用约定，不是多余的所有权转移。
#![allow(
    clippy::needless_pass_by_value,
    reason = "Tauri 命令按值接收 AppHandle"
)]

mod bridge;
mod commands;
mod session;
mod state;
mod tray;

use std::sync::Arc;

use assemble_client::{Deployment, JarvisClient};
use assemble_store::secret::SecretStore;
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;

use crate::state::{AppState, BACKGROUND_ARG, MAIN_WINDOW};

pub fn run() {
    init_logging();
    // 由登录项启动时附带 `--background`：不弹窗口，只在托盘里运行。
    let background = std::env::args()
        .skip(1)
        .any(|argument| argument == BACKGROUND_ARG);

    tauri::Builder::default()
        // 单实例守卫必须最先注册：第二个实例启动后把窗口叫到前面，然后自己退出。
        .plugin(tauri_plugin_single_instance::init(
            |app, _arguments, _current_dir| {
                state::show_main(app);
            },
        ))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![BACKGROUND_ARG]),
        ))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::snapshot,
            commands::sign_in,
            commands::cancel_sign_in,
            commands::sign_out,
            commands::launch_at_login,
            commands::set_launch_at_login,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let data_dir = handle.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            let client = JarvisClient::new(deployment())?;
            let secrets: Option<Arc<dyn SecretStore>> =
                assemble_store::secret::system_secret_store()
                    .map(|store| Arc::new(store) as Arc<dyn SecretStore>);
            handle.manage(AppState::new(handle.clone(), client, secrets));

            tray::build(&handle)?;
            open_main_window(&handle, !background)?;
            bridge::start(handle.clone(), &data_dir.join("bridge.json"));

            // 核对保存过的会话需要联网，放到后台做，不拖慢启动。
            let restore = handle.clone();
            tauri::async_runtime::spawn(async move {
                session::restore(&restore.state::<AppState>()).await;
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == MAIN_WINDOW
                && let WindowEvent::CloseRequested { api, .. } = event
            {
                // 关窗口只是收起；真正退出走托盘菜单。
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Assemble")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                // 结束所有后台任务。
                app.state::<AppState>().lifetime.cancel();
                // 显式停掉本机网关、清掉发现文件。框架托管的状态在进程结束前
                // 未必会被释放，不能指望析构来做这件事。
                if let Some(bridge) = app.try_state::<bridge::Bridge>() {
                    bridge.abort();
                }
            }
        });
}

/// 连接目标由构建类型决定。
fn deployment() -> Deployment {
    if cfg!(debug_assertions) {
        Deployment::development()
    } else {
        Deployment::production()
    }
}

fn open_main_window(app: &tauri::AppHandle, visible: bool) -> tauri::Result<()> {
    if app.get_webview_window(MAIN_WINDOW).is_none() {
        WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::App("index.html".into()))
            .title("Assemble")
            .inner_size(960.0, 640.0)
            .min_inner_size(720.0, 480.0)
            .center()
            .resizable(true)
            // 先以隐藏状态创建：后台启动时保持不出现，需要时再显示。
            .visible(false)
            .build()?;
    }
    if visible {
        state::show_main(app);
    }
    Ok(())
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;

    let default_level = if cfg!(debug_assertions) {
        "debug"
    } else {
        "info"
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    // 日志只写到标准错误；不写文件，也不记录任何凭据。
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
