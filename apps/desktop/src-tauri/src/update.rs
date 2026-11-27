//! 桌面端的自动更新：查新版、后台下载、校验签名、空闲时换上。
//!
//! 全部从 Rust 这边驱动，窗口的能力清单不变：界面只读状态里的 `update`，动手走两条具名命令
//! （`update_check`、`update_restart`）。
//!
//! 什么时候换上是一条纯规矩（[`IdleWatch`]）：主窗口收起、又没有文件在传，并且下载完之后
//! 已经这样持续了 [`IDLE_FOR`]。正在传文件的时候重启会打断传输，所以等。
//!
//! 更新包的签名由更新插件核对（公钥编在应用里，关不掉），核对不过的包装不上；这一条这里不重复做，
//! 只对着真实运行的服务端验过接得对不对（`against_server`）。

use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "windows")]
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use assemble_protocol::origin::ServerOrigin;
use assemble_protocol::updates;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_updater::UpdaterExt;

use crate::commands::CommandError;
use crate::state::{AppState, MAIN_WINDOW, Snapshot, UpdateState};

#[cfg(test)]
mod against_server;

/// 启动后这么久查第一次：不抢启动时的联网。
pub(crate) const FIRST_CHECK_AFTER: Duration = Duration::from_secs(15);
/// 之后每隔这么久查一次。
pub(crate) const CHECK_EVERY: Duration = Duration::from_hours(6);
/// 下载完之后，空闲要持续这么久才自动换上。
pub(crate) const IDLE_FOR: Duration = Duration::from_secs(180);
/// 看一眼是不是空闲的间隔。
const IDLE_POLL: Duration = Duration::from_secs(15);
/// 查清单和下载更新包各自最多等多久。
const TIMEOUT: Duration = Duration::from_secs(30);
/// "下次静默启动"的记号放了这么久还没等到重启，就当它作废：重启没成的话，
/// 下一次手动打开照常弹窗口。
const RELAUNCH_MARK_LIFETIME: Duration = Duration::from_secs(120);
const RELAUNCH_MARK: &str = "relaunch-hidden";

/// 自动更新失败时给界面的稳定错误码。
pub(crate) mod code {
    pub(crate) const UNREACHABLE: &str = "UPDATE_UNREACHABLE";
    pub(crate) const SIGNATURE: &str = "UPDATE_SIGNATURE_INVALID";
    pub(crate) const INSTALL: &str = "UPDATE_INSTALL_FAILED";
    pub(crate) const FAILED: &str = "UPDATE_FAILED";
    pub(crate) const NOT_READY: &str = "UPDATE_NOT_READY";
}

/// 这个构建会不会自动更新。
///
/// 只有装出来的应用才更新。`tauri dev` 和验收脚本跑的是 target 目录里的裸可执行文件，不是
/// 装出来的应用，版本号也一直是配置里的那个：让它更新，就是用下载来的新版本把开发用的
/// 可执行文件替换掉。装出来的应用连的是 HTTPS 的服务就更新；调试构建例外地也认回环地址，
/// 留给本机的端到端测试（那时服务端就在回环地址上）。
pub(crate) fn supported(origin: &ServerOrigin, packaged: bool, debug: bool) -> bool {
    packaged && (origin.is_secure() || debug)
}

/// 这个运行中的应用会不会自动更新。
pub(crate) fn enabled(origin: &ServerOrigin) -> bool {
    supported(origin, packaged(), cfg!(debug_assertions))
}

/// 跑的是不是装出来的应用。
///
/// macOS 上看可执行文件是不是在应用包里：`tauri build` 的产物（哪怕是调试构建）在，
/// `tauri dev` 和 `tauri build --no-bundle` 的产物都不在。别的系统没有应用包，就看是不是
/// 发布构建：那里的开发运行都是调试构建。
#[cfg(target_os = "macos")]
fn packaged() -> bool {
    std::env::current_exe().is_ok_and(|path| inside_app_bundle(&path))
}

#[cfg(not(target_os = "macos"))]
fn packaged() -> bool {
    !cfg!(debug_assertions)
}

#[cfg(target_os = "macos")]
fn inside_app_bundle(path: &std::path::Path) -> bool {
    path.components().any(|component| {
        std::path::Path::new(component.as_os_str())
            .extension()
            .is_some_and(|extension| extension == "app")
    })
}

/// 现在换不换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    Wait,
    Apply,
}

/// 现在有没有事不能被打断：用户挑好了文件还没发、有文件正在排队或收发、有网络下载在跑。
pub(crate) fn busy(snapshot: &Snapshot) -> bool {
    snapshot
        .remote
        .sessions
        .iter()
        .any(|session| session.phase != assemble_protocol::remote::RemotePhase::Ended)
        || snapshot.draft.is_some()
        || snapshot.transfers.iter().any(|transfer| {
            matches!(
                transfer.phase,
                crate::registry::TransferPhase::Queued
                    | crate::registry::TransferPhase::InProgress
                    | crate::registry::TransferPhase::Waiting
            )
        })
        || snapshot
            .downloads
            .tasks
            .iter()
            .any(|task| task.phase.active())
}

fn decide(window_hidden: bool, busy: bool, ready_for: Duration) -> Decision {
    if window_hidden && !busy && ready_for >= IDLE_FOR {
        Decision::Apply
    } else {
        Decision::Wait
    }
}

/// 记下"从什么时候起一直空闲"。中间一忙，或者窗口又被打开，就从头算。
#[derive(Debug, Default)]
pub(crate) struct IdleWatch {
    since: Option<Instant>,
}

impl IdleWatch {
    /// 看一眼。空闲持续够久了就回 [`Decision::Apply`]。
    pub(crate) fn observe(&mut self, window_hidden: bool, busy: bool, now: Instant) -> Decision {
        if !window_hidden || busy {
            self.since = None;
            return Decision::Wait;
        }
        let since = *self.since.get_or_insert(now);
        decide(window_hidden, busy, now.saturating_duration_since(since))
    }
}

/// 更新这一路的现场。
#[derive(Default)]
pub(crate) struct Pending {
    /// Windows 上留着下载好的安装包，等空闲或退出时装。
    #[cfg(target_os = "windows")]
    package: Mutex<Option<Package>>,
    /// 是不是已经有一轮在查、在下载。托盘和界面同时点也只跑一轮。
    running: AtomicBool,
}

#[cfg(target_os = "windows")]
struct Package {
    update: tauri_plugin_updater::Update,
    bytes: Vec<u8>,
}

enum Outcome {
    UpToDate,
    Fetched { version: String },
}

/// 启动两条后台循环：定时查、下载；下载完之后盯着，空闲了就换上。不自动更新的构建什么都不起。
pub fn start(app: &AppHandle) {
    let enabled = {
        let state = app.state::<AppState>();
        enabled(state.client.origin())
    };
    if !enabled {
        return;
    }
    app.manage(Pending::default());
    let handle = app.clone();
    tauri::async_runtime::spawn(async move { supervise(handle).await });
    let handle = app.clone();
    tauri::async_runtime::spawn(async move { watch_for_idle(handle).await });
}

/// 上次是为了换新版才重启的吗？记号用一次就删；放了太久的不算。
pub(crate) fn relaunch_queued<R: Runtime>(app: &AppHandle<R>) -> bool {
    let Ok(directory) = app.path().app_data_dir() else {
        return false;
    };
    let path = directory.join(RELAUNCH_MARK);
    let queued = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.trim().parse::<u128>().ok())
        .and_then(|millis| u64::try_from(millis).ok())
        .is_some_and(|millis| {
            now_millis().saturating_sub(millis)
                < u64::try_from(RELAUNCH_MARK_LIFETIME.as_millis()).unwrap_or(u64::MAX)
        });
    let _ = std::fs::remove_file(&path);
    queued
}

async fn supervise(app: AppHandle) {
    let lifetime = app.state::<AppState>().lifetime.clone();
    tokio::select! {
        () = lifetime.cancelled() => return,
        () = tokio::time::sleep(FIRST_CHECK_AFTER) => {}
    }
    loop {
        cycle(&app).await;
        tokio::select! {
            () = lifetime.cancelled() => return,
            () = tokio::time::sleep(CHECK_EVERY) => {}
        }
    }
}

async fn watch_for_idle(app: AppHandle) {
    let lifetime = app.state::<AppState>().lifetime.clone();
    let mut watch = IdleWatch::default();
    let mut ticker = tokio::time::interval(IDLE_POLL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    loop {
        tokio::select! {
            () = lifetime.cancelled() => return,
            _ = ticker.tick() => {}
        }
        let state = app.state::<AppState>();
        if state.update_view().state != UpdateState::Ready {
            watch = IdleWatch::default();
            continue;
        }
        let hidden = main_window_hidden(&app);
        let busy = busy(&state.snapshot());
        if watch.observe(hidden, busy, Instant::now()) == Decision::Apply {
            match apply(&app, true) {
                // 重启已经提出去了：不再看，免得又提一次。
                Ok(()) => return,
                Err(error) => {
                    tracing::warn!(code = error.code, message = %error.message, "the update could not be applied; it will be retried");
                    watch = IdleWatch::default();
                }
            }
        }
    }
}

/// 查一次，有新版就下载（macOS 上接着就把应用包换掉）。已经在跑的时候什么都不做，直接回来。
pub(crate) async fn cycle(app: &AppHandle) {
    // 不自动更新的构建没有这一路的现场。
    let Some(pending) = app.try_state::<Pending>() else {
        return;
    };
    let pending: &Pending = &pending;
    if pending.running.swap(true, Ordering::SeqCst) {
        return;
    }
    let _running = Running(&pending.running);

    let state = app.state::<AppState>();
    state.set_update(|view| {
        view.state = UpdateState::Checking;
        view.error = None;
        view.received = None;
        view.total = None;
    });
    let outcome = check_and_fetch(app, pending).await;
    // 下载好那一种在 check_and_fetch 里已经记过；另外两种在这里记，日志里看得出每一轮的结果。
    match &outcome {
        Ok(Outcome::UpToDate) => tracing::info!("no newer version has been published"),
        Err(code) => {
            tracing::warn!(code = %code, "the update check did not succeed; it will be retried");
        }
        Ok(Outcome::Fetched { .. }) => {}
    }
    let checked_at = i64::try_from(now_millis()).ok();
    state.set_update(|view| {
        view.checked_at = checked_at;
        match outcome {
            Ok(Outcome::UpToDate) => {
                view.state = UpdateState::UpToDate;
                view.version = None;
            }
            Ok(Outcome::Fetched { version }) => {
                view.state = UpdateState::Ready;
                view.version = Some(version);
            }
            Err(error) => {
                view.state = UpdateState::Failed;
                view.error = Some(error);
            }
        }
    });
    crate::tray::refresh(app);
}

/// 一轮查、下载进行期间一直持有：无论是怎么离开这一段代码的，"正在跑"都要放回去。
struct Running<'a>(&'a AtomicBool);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

async fn check_and_fetch(app: &AppHandle, pending: &Pending) -> Result<Outcome, String> {
    let endpoint = {
        let state = app.state::<AppState>();
        updates::manifest_url(state.client.origin())
    };
    let url = tauri::Url::parse(&endpoint).map_err(|_| code::FAILED.to_owned())?;
    let updater = app
        .updater_builder()
        .endpoints(vec![url])
        .map_err(|_| code::FAILED.to_owned())?
        .timeout(TIMEOUT)
        .build()
        .map_err(|_| code::FAILED.to_owned())?;
    let update = match updater.check().await {
        Ok(update) => update,
        // 服务端还没有发布过，或者清单里没有这个平台：没有新版，不是出错。
        Err(error) if code_of(&error).is_none() => return Ok(Outcome::UpToDate),
        Err(error) => return Err(code_of(&error).unwrap_or(code::FAILED).to_owned()),
    };
    let Some(update) = update else {
        return Ok(Outcome::UpToDate);
    };
    let version = update.version.clone();
    tracing::info!(version = %version, "a new version was published; downloading it");
    let progress = app.clone();
    let shown = version.clone();
    let bytes = update
        .download(
            move |received, total| {
                progress.state::<AppState>().set_update(|view| {
                    view.state = UpdateState::Downloading;
                    view.version = Some(shown.clone());
                    view.received = Some(received as u64);
                    view.total = total;
                });
            },
            || {},
        )
        .await
        .map_err(|error| code_of(&error).unwrap_or(code::FAILED).to_owned())?;
    tracing::info!(version = %version, bytes = bytes.len(), "the update package was downloaded and verified");

    #[cfg(target_os = "macos")]
    {
        // 应用包当场换掉，运行中的进程不受影响；下一次启动（或者稍后重启）就是新版。
        let _ = pending;
        update
            .install(&bytes)
            .map_err(|_| code::INSTALL.to_owned())?;
        Ok(Outcome::Fetched { version })
    }
    #[cfg(target_os = "windows")]
    {
        // 安装器要等应用退出才能跑：先留着。
        pending
            .package
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(Package { update, bytes });
        Ok(Outcome::Fetched { version })
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (pending, update, bytes);
        Err(code::INSTALL.to_owned())
    }
}

/// 换上已下载的新版并重启。`quiet`：重启之后只在托盘里，不弹窗口。
/// Windows 上安装器起来之后进程就结束了，这一步不回来；macOS 上提出重启的请求就回来。
pub(crate) fn apply(app: &AppHandle, quiet: bool) -> Result<(), CommandError> {
    let version = ready_version(app).ok_or_else(not_ready)?;
    if quiet {
        queue_relaunch(app);
    }
    #[cfg(target_os = "macos")]
    {
        tracing::info!(version = %version, "restarting into the new version");
        // 不直接 restart()：从主线程（托盘菜单）调它时 Tauri 不发退出事件，退出时要做的清理就被
        // 跳过了。这里只提出退出的请求，事件循环照常发退出事件、做完清理，然后才重启。
        app.request_restart();
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        let package = app
            .state::<Pending>()
            .package
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or_else(not_ready)?;
        tracing::info!(version = %version, "installing the new version; the app will be closed");
        // 安装器起来之后进程就直接结束，不会走到退出事件：退出时要做的清理先在这里做掉。
        crate::stop(app);
        package
            .update
            .restart_after_install(true)
            .install(&package.bytes)
            .map_err(|_| CommandError::local(code::INSTALL, "the installer could not be started"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = app;
        Err(CommandError::local(
            code::INSTALL,
            "updates are not supported on this platform",
        ))
    }
}

/// 用户从托盘退出：有下载好的更新就趁退出时装上，不重新拉起（macOS 上早就换好了）。
/// 装不上就照常退出。
pub(crate) fn quit(app: &AppHandle) {
    #[cfg(target_os = "windows")]
    if let Some(package) = take_package(app) {
        tracing::info!("installing the downloaded update before quitting");
        crate::stop(app);
        if let Err(error) = package
            .update
            .restart_after_install(false)
            .install(&package.bytes)
        {
            tracing::warn!(%error, "the downloaded update could not be installed; quitting anyway");
        }
    }
    app.exit(0);
}

#[cfg(target_os = "windows")]
fn take_package(app: &AppHandle) -> Option<Package> {
    let pending = app.try_state::<Pending>()?;
    pending
        .package
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
}

fn ready_version(app: &AppHandle) -> Option<String> {
    let view = app.state::<AppState>().update_view();
    if view.state == UpdateState::Ready {
        view.version
    } else {
        None
    }
}

fn queue_relaunch(app: &AppHandle) {
    let Ok(directory) = app.path().app_data_dir() else {
        return;
    };
    let _ = std::fs::write(directory.join(RELAUNCH_MARK), now_millis().to_string());
}

fn main_window_hidden(app: &AppHandle) -> bool {
    // 问不到就当它开着：拿不准的时候不重启。
    app.get_webview_window(MAIN_WINDOW)
        .is_none_or(|window| !window.is_visible().unwrap_or(true))
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |passed| {
            u64::try_from(passed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn not_ready() -> CommandError {
    CommandError::local(code::NOT_READY, "no update has been downloaded")
}

/// 插件的错误换成稳定错误码。`None` 表示"没有新版"，不是出错。
///
/// 包不是那把钥匙签的、签完之后被改过、不是为清单上的那个版本签的、签名里没写版本——对用户来说是
/// 同一件事：这个包不可信，不装。所以都给签名那一个错误码。
fn code_of(error: &tauri_plugin_updater::Error) -> Option<&'static str> {
    use tauri_plugin_updater::Error as Failure;
    match error {
        Failure::ReleaseNotFound | Failure::TargetNotFound(_) | Failure::TargetsNotFound(_) => None,
        Failure::Minisign(_)
        | Failure::Base64(_)
        | Failure::SignatureUtf8(_)
        | Failure::SignedVersionMismatch { .. }
        | Failure::MissingSignedVersion => Some(code::SIGNATURE),
        Failure::Network(_) | Failure::Reqwest(_) => Some(code::UNREACHABLE),
        _ => Some(code::FAILED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::TransferPhase as Phase;

    fn origin(text: &str) -> ServerOrigin {
        ServerOrigin::parse(text).unwrap()
    }

    fn seconds(count: u64) -> Duration {
        Duration::from_secs(count)
    }

    #[test]
    fn only_packaged_builds_pointing_at_a_secure_service_update_themselves() {
        assert!(supported(&origin("https://as.yxswy.com"), true, false));
        // 连回环地址的发布构建不更新。
        assert!(!supported(&origin("http://127.0.0.1:8787"), true, false));
        // 调试构建例外：本机的端到端测试要对着回环地址走一遍。
        assert!(supported(&origin("http://127.0.0.1:8787"), true, true));
    }

    #[test]
    fn development_runs_never_update_themselves() {
        // 开发运行连的是正式地址也不更新：它的可执行文件不该被下载来的新版本替换掉。
        assert!(!supported(&origin("https://as.yxswy.com"), false, false));
        assert!(!supported(&origin("http://127.0.0.1:8787"), false, true));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn only_an_executable_inside_a_bundle_counts_as_packaged_on_macos() {
        assert!(inside_app_bundle(std::path::Path::new(
            "/Applications/Assemble.app/Contents/MacOS/assemble-desktop"
        )));
        assert!(inside_app_bundle(std::path::Path::new(
            "target/debug/bundle/macos/Assemble.app/Contents/MacOS/assemble-desktop"
        )));
        assert!(!inside_app_bundle(std::path::Path::new(
            "/Users/me/Assemble/target/debug/assemble-desktop"
        )));
        assert!(!inside_app_bundle(std::path::Path::new(
            "/tmp/Assemble.app.old/assemble-desktop"
        )));
    }

    #[test]
    fn the_update_is_applied_only_after_three_idle_minutes() {
        assert_eq!(decide(false, false, seconds(0)), Decision::Wait);
        assert_eq!(decide(true, true, seconds(0)), Decision::Wait);
        assert_eq!(decide(true, false, seconds(179)), Decision::Wait);
        assert_eq!(decide(true, false, seconds(180)), Decision::Apply);
    }

    #[test]
    fn being_busy_or_opening_the_window_starts_the_idle_time_over() {
        let start = Instant::now();
        let mut watch = IdleWatch::default();
        assert_eq!(watch.observe(true, false, start), Decision::Wait);
        assert_eq!(
            watch.observe(true, false, start + seconds(120)),
            Decision::Wait
        );
        // 窗口被打开了一次：从头算。
        assert_eq!(
            watch.observe(false, false, start + seconds(130)),
            Decision::Wait
        );
        assert_eq!(
            watch.observe(true, false, start + seconds(200)),
            Decision::Wait
        );
        assert_eq!(
            watch.observe(true, false, start + seconds(380)),
            Decision::Apply
        );

        // 有文件在传：从头算，哪怕窗口收着。
        let mut watch = IdleWatch::default();
        assert_eq!(watch.observe(true, false, start), Decision::Wait);
        assert_eq!(
            watch.observe(true, true, start + seconds(100)),
            Decision::Wait
        );
        assert_eq!(
            watch.observe(true, false, start + seconds(250)),
            Decision::Wait
        );
        assert_eq!(
            watch.observe(true, false, start + seconds(430)),
            Decision::Apply
        );
    }

    #[test]
    fn work_in_flight_counts_as_busy() {
        let samples: Vec<Snapshot> =
            serde_json::from_str(include_str!("../../contract/snapshots.json")).unwrap();
        // 一条已经下完的下载不算在忙。
        let mut snapshot = samples[0].clone();
        snapshot.downloads.tasks[0].phase = assemble_protocol::downloads::DownloadPhase::Completed;
        assert!(!busy(&snapshot));

        snapshot.downloads.tasks[0].phase =
            assemble_protocol::downloads::DownloadPhase::Downloading;
        assert!(busy(&snapshot), "网络下载在跑");
        snapshot.downloads.tasks[0].phase = assemble_protocol::downloads::DownloadPhase::Completed;

        let transfer = samples[7].transfers[0].clone();
        for (phase, expected) in [
            (Phase::Queued, true),
            (Phase::InProgress, true),
            (Phase::Waiting, true),
            (Phase::Complete, false),
            (Phase::Failed, false),
            (Phase::Declined, false),
            (Phase::Cancelled, false),
            (Phase::Expired, false),
        ] {
            snapshot.transfers = vec![transfer.clone()];
            snapshot.transfers[0].phase = phase;
            assert_eq!(busy(&snapshot), expected, "{phase:?}");
        }

        // 用户挑好了文件还没发。
        snapshot.transfers = Vec::new();
        snapshot.draft = samples[10].draft.clone();
        assert!(busy(&snapshot));
    }

    #[test]
    fn remote_sessions_prevent_idle_restart() {
        let samples: Vec<Snapshot> =
            serde_json::from_str(include_str!("../../contract/snapshots.json")).unwrap();
        for mut sample in samples
            .into_iter()
            .filter(|s| !s.remote.sessions.is_empty())
        {
            sample.draft = None;
            sample.transfers.clear();
            sample.downloads.tasks.clear();
            match sample.remote.sessions[0].phase {
                assemble_protocol::remote::RemotePhase::Ended => {
                    assert!(!busy(&sample), "已结束的会话不能阻止更新");
                }
                _ => assert!(busy(&sample), "请求中和活动会话不能被更新打断"),
            }
            sample.remote.sessions.clear();
            assert!(!busy(&sample));
        }
    }

    #[test]
    fn the_app_and_the_server_pin_the_same_public_key() {
        // 公钥有两份：这里给更新插件验下载下来的包，assemble_protocol 里那份给服务端在发布时核对。
        // 两份对不上的话，服务端放出去的包应用会拒收（或者反过来），所以钉在一起。
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            config["plugins"]["updater"]["pubkey"],
            assemble_protocol::updates::PUBLIC_KEY
        );
        assert_eq!(config["plugins"]["updater"]["requireSignedVersion"], true);
    }

    #[test]
    fn a_missing_release_is_not_an_error() {
        use tauri_plugin_updater::Error as Failure;
        assert_eq!(code_of(&Failure::ReleaseNotFound), None);
        assert_eq!(
            code_of(&Failure::TargetNotFound("darwin-aarch64".into())),
            None
        );
        assert_eq!(
            code_of(&Failure::SignatureUtf8("x".into())),
            Some(code::SIGNATURE)
        );
        // 签名是为别的版本签的、签名里没写版本：同样是这个包不可信。
        assert_eq!(
            code_of(&Failure::SignedVersionMismatch {
                signed: "0.1.1".into(),
                announced: "0.1.2".into(),
            }),
            Some(code::SIGNATURE)
        );
        assert_eq!(
            code_of(&Failure::MissingSignedVersion),
            Some(code::SIGNATURE)
        );
        assert_eq!(
            code_of(&Failure::Network("x".into())),
            Some(code::UNREACHABLE)
        );
        assert_eq!(
            code_of(&Failure::Io(std::io::Error::other("x"))),
            Some(code::FAILED)
        );
    }
}
