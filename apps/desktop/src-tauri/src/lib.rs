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
mod launch;
mod notify;
mod planner;
mod registry;
mod remote;
mod runtime;
mod send_menu;
mod send_panel;
mod session;
mod state;
mod system_notifications;
mod transfer_history;
mod transfers;
mod tray;
mod update;

use std::io::{self, Write};
use std::sync::{Arc, Mutex, PoisonError};

use assemble_client::{Client, ServerOrigin};
use assemble_protocol::origin::OriginError;
use assemble_store::logfile::LogFile;
use assemble_store::secret::SecretStore;
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;
use tracing_subscriber::fmt::MakeWriter;

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
    let sink = Sink::default();
    init_logging(sink.clone());
    let client = Client::new(deployment()?)?;
    // 由登录项启动时附带 `--background`：不弹窗口，只在托盘里运行。
    let background = std::env::args().skip(1).any(|arg| arg == BACKGROUND_ARG);
    // 由右键菜单"用 Assemble 发送"启动时带着文件：等会话核对完再打开发送面板。
    let handed_over = launch::files_at_startup(std::env::args_os());

    tauri::Builder::default()
        // 单实例守卫必须最先注册：应用已经在运行时，第二个实例把参数转交给它，然后自己退出。
        // 带着文件来的（右键菜单）就打开发送面板；否则把窗口叫到前面。
        .plugin(tauri_plugin_single_instance::init(
            |app, arguments, current_dir| {
                launch::forwarded(app, &arguments, &current_dir);
            },
        ))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![BACKGROUND_ARG]),
        ))
        .plugin(tauri_plugin_opener::init())
        // 对话框与通知只从 Rust 这边用：窗口的能力清单不变，界面拿不到它们。
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        // 自动更新也一样只从 Rust 这边用：界面读状态、调两条具名命令，拿不到更新插件。
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(handlers())
        .setup(move |app| start(app, client, background, handed_over, sink))
        .on_window_event(|window, event| {
            // 把文件拖进主窗口：交给发送面板。位置由系统直接交给 Rust，不经过界面。
            if window.label() == MAIN_WINDOW
                && let WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event
            {
                launch::dropped(window.app_handle(), paths.clone());
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                match window.label() {
                    // 关窗口只是收起；真正退出走托盘菜单。
                    MAIN_WINDOW => {
                        api.prevent_close();
                        remote::close_window(window.app_handle());
                        let _ = window.hide();
                    }
                    // 发送面板关了就真的关掉；挑好却没发的文件不再留着。
                    send_panel::SEND_WINDOW => send_panel::closed(window.app_handle()),
                    _ => {}
                }
            }
        })
        .build(context())
        .map_err(StartupError::from)?
        .run(|app, event| {
            if let RunEvent::Exit = event {
                stop(app);
            }
        });
    Ok(())
}

/// 这个应用自己的配置：`tauri.conf.json` 加上编进去的界面资源。
/// 测试里带着更新插件的模拟应用也用它：那个插件要读到配置里 `plugins.updater` 那一段才肯初始化。
/// 宏只能展开一次（它定义的符号不能重复），所以两处共用这一个函数。
fn context<R: tauri::Runtime>() -> tauri::Context<R> {
    tauri::generate_context!()
}

/// 先停入口，再等待下载和原生宿主连接收尾，最后删除本实例的临时桥接凭据。
fn stop(app: &tauri::AppHandle) {
    app.state::<AppState>().lifetime.cancel();
    tauri::async_runtime::block_on(app.state::<AppState>().remote_runtime.stop(None));
    let bridge = app.try_state::<Option<assemble_download::browser::BrowserServer>>();
    if let Some(bridge) = bridge.as_ref().and_then(|state| state.inner().as_ref()) {
        bridge.cancel();
    }
    tauri::async_runtime::block_on(
        app.state::<Arc<assemble_download::DownloadManager>>()
            .shutdown(),
    );
    if let Some(bridge) = bridge.as_ref().and_then(|state| state.inner().as_ref()) {
        tauri::async_runtime::block_on(bridge.shutdown());
    }
    if app.state::<AppState>().flush_transfer_history().is_err() {
        tracing::warn!("the transfer history could not be saved before exit");
    }
}

// 命令清单单独集中，启动函数只保留生命周期编排。
fn handlers() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        commands::remote_request,
        commands::remote_approve,
        commands::remote_end,
        commands::remote_poll,
        commands::remote_inputs,
        commands::remote_fullscreen,
        commands::memo_list,
        commands::memo_get,
        commands::memo_create,
        commands::memo_update,
        commands::memo_remove,
        commands::memo_copy,
        commands::media_capabilities,
        commands::media_list,
        commands::media_get,
        commands::media_create,
        commands::media_update,
        commands::media_remove,
        commands::media_search,
        commands::media_catalog,
        commands::media_actor,
        commands::media_import,
        commands::media_job,
        commands::media_gallery,
        commands::media_sync_gallery,
        commands::media_select_poster,
        commands::media_image,
        commands::media_copy_magnet,
        commands::snapshot,
        commands::download_set_assistant_allowed,
        commands::download_prepare,
        commands::download_import_hls,
        commands::download_start,
        commands::download_cancel,
        commands::download_retry,
        commands::download_choose_folder,
        commands::download_open,
        commands::download_reveal,
        commands::sign_in,
        commands::cancel_sign_in,
        commands::sign_out,
        commands::launch_at_login,
        commands::set_launch_at_login,
        commands::set_send_menu,
        commands::rename_device,
        commands::remove_device,
        commands::choose_receive_folder,
        commands::reset_receive_folder,
        commands::reveal_received,
        commands::start_send,
        commands::send_to_device,
        commands::send_to_drive,
        commands::drive_refresh,
        commands::drive_upload,
        commands::drive_download,
        commands::drive_rename,
        commands::drive_delete,
        commands::close_send_panel,
        commands::cancel_transfer,
        commands::retry_transfer,
        commands::open_main,
        commands::open_public_media,
        commands::dismiss_notice,
        commands::update_check,
        commands::update_restart,
        commands::system_notifications_refresh,
        commands::system_notifications_read,
        commands::planner_refresh,
        commands::schedule_create,
        commands::schedule_update,
        commands::schedule_complete,
        commands::schedule_cancel,
        commands::schedule_reopen,
        commands::schedule_delete,
        commands::reminder_create,
        commands::reminder_update,
        commands::reminder_dismiss,
        commands::reminder_cancel,
        commands::reminder_delete,
        commands::notifications_read,
        commands::notifications_read_all,
        commands::planner_focus,
        commands::planner_compose,
        commands::set_planner_muted,
    ]
}

/// 应用启动：把状态装好、托盘与窗口立起来、后台任务跑起来。
fn start(
    app: &mut tauri::App,
    client: Client,
    background: bool,
    handed_over: Option<Vec<std::path::PathBuf>>,
    sink: Sink,
) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    std::fs::create_dir_all(handle.path().app_data_dir()?)?;
    // 日志文件放在系统给应用留的日志目录里（macOS 是 ~/Library/Logs/<应用标识>，
    // Windows 是 %LOCALAPPDATA%\<应用标识>\logs）。从启动台或托盘启动的进程没有可见的
    // 错误输出，出了事全靠它。
    sink.attach(
        handle
            .path()
            .app_log_dir()
            .map_err(|error| io::Error::other(error.to_string()))
            .and_then(|directory| LogFile::open(&directory, "desktop.log")),
    );
    // 记的是应用自己的版本（自动更新拿它比先后），不是 cargo 包的那个 0.1.0。
    tracing::info!(
        version = %app.package_info().version,
        "Assemble desktop is starting; the local log file is attached unless a warning says otherwise"
    );

    let secrets: Option<Arc<dyn SecretStore>> = assemble_store::secret::system_secret_store()
        .map(|store| Arc::new(store) as Arc<dyn SecretStore>);
    let settings = transfers::load_settings(&handle);
    let notifier = Arc::new(notify::SystemNotifier::new(handle.clone()));
    let browser_secrets = secrets.clone();
    // 上次是为了换新版才重启的话，这一次照样不弹窗口。记号每次启动都要读掉，本来就是后台启动时
    // 也一样：不读它会留在那里，两分钟内的下一次启动就被当成"这一回也不弹窗口"。
    let relaunching = update::relaunch_queued(&handle);
    let background = background || relaunching;
    handle.manage(AppState::new(
        handle.clone(),
        client,
        secrets,
        settings,
        transfers::system_downloads_dir(&handle),
        notifier,
    ));
    let downloads_handle = handle.clone();
    handle
        .state::<AppState>()
        .attach_transfer_history(handle.path().app_data_dir()?.join("transfers.json"))?;
    handle.state::<AppState>().attach_system_notifications(
        handle
            .path()
            .app_data_dir()?
            .join("system-notifications.json"),
    );
    let downloads = assemble_download::DownloadManager::open(
        handle.path().app_data_dir()?.join("network-downloads.json"),
        transfers::system_downloads_dir(&handle)
            .join("Assemble")
            .join("网络下载"),
        assemble_download::MediaTools::discover(),
        move |view| downloads_handle.state::<AppState>().set_downloads(view),
    )?;
    downloads.set_assistant_allowed(
        handle
            .state::<AppState>()
            .settings()
            .assistant_downloads_allowed
            .unwrap_or(true),
    );
    handle.state::<AppState>().set_downloads(downloads.view());
    handle.manage(Arc::clone(&downloads));
    // 上次崩溃留下的下载暂存目录趁现在清掉：窗口还没开出来，不可能有任务在跑。
    // 平时这里什么都不用删；真留下来几个 G 的分片时，等它删完比留着占地方强。
    tauri::async_runtime::block_on(downloads.sweep_staging());

    tray::build(&handle)?;
    // 点通知的处理要早于第一条通知登记好，否则点了也报不上来。
    notify::install(&handle);
    open_main_window(&handle, !background)?;
    // 窗口成功建立后才公布原生入口，避免启动中途失败留下可连接的桥接。
    let browser_window = handle.clone();
    let browser = browser_secrets.and_then(|store| {
        tauri::async_runtime::block_on(assemble_download::browser::BrowserServer::start(
            downloads,
            store,
            &handle.config().identifier,
            move || state::show_main(&browser_window),
        ))
        .ok()
    });
    handle.manage(browser);

    runtime::start(&handle);
    update::start(&handle);
    // 让 Windows 的右键菜单指向这个可执行文件（别的运行方式下什么都不做）。
    send_menu::sync(&handle.state::<AppState>().settings());

    // 核对保存过的会话需要联网，放到后台做，不拖慢启动。
    let restore = handle.clone();
    tauri::async_runtime::spawn(async move {
        session::restore(&restore.state::<AppState>()).await;
        // 由右键菜单带着文件启动的：现在才知道登录了没有，可以打开发送面板了。
        if let Some(files) = handed_over
            && let Err(error) = send_panel::start_with(&restore, files).await
        {
            send_panel::report(&restore, &error);
            state::show_main(&restore);
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

/// 日志写到哪儿：标准错误，加上应用启动之后的一个本机文件。
///
/// 全局的日志分发只能装一次，所以写入口在 Tauri 起来之前就得交出去；那时还不知道
/// 应用的目录在哪，文件要晚一步才接上。接上之前的那些只写标准错误。
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Option<LogFile>>>);

impl Sink {
    /// 把本机日志文件接上。打不开就只写标准错误，应用照常运行。
    fn attach(&self, opened: io::Result<LogFile>) {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match opened {
            Ok(log) => *slot = Some(log),
            Err(error) => tracing::warn!(
                %error,
                "the local log file could not be opened; logs stay on stderr"
            ),
        }
    }
}

impl<'a> MakeWriter<'a> for Sink {
    type Writer = SinkWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        SinkWriter(self)
    }
}

/// 一次写入的两头：标准错误，和接上之后的本机文件。
struct SinkWriter<'a>(&'a Sink);

impl Write for SinkWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        std::io::stderr().write_all(buf)?;
        if let Some(log) = self
            .0
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            // 文件写不进去不算日志失败：标准错误上那一份已经写过了。
            let _ = log.write(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        std::io::stderr().flush()
    }
}

fn init_logging(sink: Sink) {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::prelude::*;

    let default_level = if cfg!(debug_assertions) {
        "debug"
    } else {
        "info"
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    // 同时写标准错误和一个限大小的本机文件。两边都不记录任何凭据——这是各处写日志时
    // 守的规矩。不带颜色：文件里留着那串转义码没法读。
    // enigo 的 debug 日志会逐字打印输入；即使环境显式要求 trace 也不能记录它。
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::filter::filter_fn(|meta| {
            allowed_log_target(meta.target())
        }))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(sink),
        )
        .try_init();
}

fn allowed_log_target(target: &str) -> bool {
    target != "enigo" && !target.starts_with("enigo::")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_input_library_logs_are_filtered_even_at_trace() {
        use tracing_subscriber::prelude::*;
        let dir = tempfile::tempdir().unwrap();
        let sink = Sink::default();
        sink.attach(LogFile::open(dir.path(), "desktop.log"));
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new("trace"))
            .with(tracing_subscriber::filter::filter_fn(|meta| {
                allowed_log_target(meta.target())
            }))
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(sink),
            );
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!(target:"enigo::win::win_impl","synthetic-input-must-not-appear");
            tracing::info!(target:"enigo","synthetic-input-must-not-appear");
            tracing::info!(target:"assemble_desktop_lib","normal-diagnostic");
        });
        let logged = std::fs::read_to_string(dir.path().join("desktop.log")).unwrap();
        assert!(!logged.contains("synthetic-input"));
        assert!(logged.contains("normal-diagnostic"));
    }

    #[test]
    fn what_reaches_stderr_also_reaches_the_file_once_it_is_attached() {
        let dir = tempfile::tempdir().unwrap();
        let sink = Sink::default();
        // 还没接上文件：照常写，不出错。
        writeln!(sink.make_writer(), "before").unwrap();
        // 接上之后：每一行都进了文件。
        sink.attach(LogFile::open(dir.path(), "desktop.log"));
        let mut writer = sink.make_writer();
        writer.write_all(b"first line\n").unwrap();
        writer.write_all(b"second line\n").unwrap();
        writer.flush().unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desktop.log")).unwrap(),
            "first line\nsecond line\n"
        );
        // 打不开文件也不碍事：只写标准错误。
        sink.attach(Err(io::Error::other("no such directory")));
        writeln!(sink.make_writer(), "still fine").unwrap();
    }

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
