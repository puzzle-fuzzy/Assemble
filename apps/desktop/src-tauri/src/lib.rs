//! Assemble 桌面壳。
//!
//! 这里只做"壳"的事：窗口、托盘、开机启动、生命周期，以及把 Rust 核心的能力
//! 以少量具名命令的形式交给界面。业务规则都在 `crates/` 里，可以脱离界面测试。
//!
//! 连接哪个服务由构建决定：服务地址不从界面输入，也不从用户配置读取。

// Tauri 命令按值接收 `AppHandle`，这是框架的调用约定，不是多余的所有权转移。
#![allow(
    clippy::needless_pass_by_value,
    reason = "Tauri 命令按值接收 AppHandle"
)]

mod commands;
mod runtime;
mod session;
mod state;
mod tray;

use std::sync::Arc;

use assemble_client::{Client, ServerOrigin};
use assemble_protocol::origin::OriginError;
use assemble_store::secret::SecretStore;
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;

use crate::state::{AppState, BACKGROUND_ARG, MAIN_WINDOW};

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("this build has no server configured; build it with ASSEMBLE_SERVER_ORIGIN set")]
    NotConfigured,
    #[error("ASSEMBLE_SERVER_ORIGIN is invalid: {0}")]
    InvalidOrigin(#[from] OriginError),
    #[error("the HTTP client could not be created: {0}")]
    Client(#[from] assemble_client::ClientBuildError),
    #[error(transparent)]
    Tauri(#[from] tauri::Error),
}

pub fn run() -> Result<(), StartupError> {
    init_logging();
    let client = Client::new(deployment()?)?;
    // 由登录项启动时附带 `--background`：不弹窗口，只在托盘里运行。
    let background = std::env::args().skip(1).any(|arg| arg == BACKGROUND_ARG);

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
            commands::rename_device,
            commands::remove_device,
            commands::dismiss_notice,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            std::fs::create_dir_all(handle.path().app_data_dir()?)?;

            let secrets: Option<Arc<dyn SecretStore>> =
                assemble_store::secret::system_secret_store()
                    .map(|store| Arc::new(store) as Arc<dyn SecretStore>);
            handle.manage(AppState::new(handle.clone(), client, secrets));

            tray::build(&handle)?;
            open_main_window(&handle, !background)?;
            runtime::start(&handle);

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
        .map_err(StartupError::from)?
        .run(|app, event| {
            if let RunEvent::Exit = event {
                // 结束所有后台任务。框架托管的状态在进程结束前未必会被释放，
                // 不能指望析构来做这件事。
                app.state::<AppState>().lifetime.cancel();
            }
        });
    Ok(())
}

/// 连接目标由构建决定。
///
/// 设置了 `ASSEMBLE_SERVER_ORIGIN` 就连它（调试与发布构建都认）。
fn deployment() -> Result<ServerOrigin, StartupError> {
    // 编译期读取的值只可能是 `Some("")` 或一段真实地址，不会是 `None` 之外的空值。
    choose_origin(
        option_env!("ASSEMBLE_SERVER_ORIGIN").filter(|value| !value.is_empty()),
        cfg!(debug_assertions),
    )
}

/// 回退规则：没配置地址时，调试构建连本地开发服务；发布构建拒绝启动——
/// 宁可起不来，也不连到一个猜出来的地址。
fn choose_origin(configured: Option<&str>, debug: bool) -> Result<ServerOrigin, StartupError> {
    match configured {
        Some(origin) => Ok(ServerOrigin::parse(origin)?),
        None if debug => Ok(ServerOrigin::development()),
        None => Err(StartupError::NotConfigured),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configured_origin_wins_over_both_fallbacks() {
        let configured = choose_origin(Some("https://assemble.example"), true).unwrap();
        assert_eq!(configured.as_str(), "https://assemble.example");
        let release = choose_origin(Some("https://assemble.example"), false).unwrap();
        assert_eq!(release, configured);
        // 配置错了就启动失败，错误里说清楚是哪个变量。
        let error = choose_origin(Some("http://assemble.example"), false).unwrap_err();
        assert!(matches!(error, StartupError::InvalidOrigin(_)));
        assert!(error.to_string().contains("ASSEMBLE_SERVER_ORIGIN"));
    }

    #[test]
    fn without_a_configured_origin_a_debug_build_uses_the_dev_server_and_a_release_build_refuses() {
        assert_eq!(
            choose_origin(None, true).unwrap().as_str(),
            "http://127.0.0.1:8787"
        );
        let error = choose_origin(None, false).unwrap_err();
        assert!(matches!(error, StartupError::NotConfigured));
        assert!(error.to_string().contains("ASSEMBLE_SERVER_ORIGIN"));
    }
}
