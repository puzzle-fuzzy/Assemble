//! 应用状态：服务是否可达、会话处在哪个阶段、有哪些设备，以及这些状态如何交给界面。
//!
//! 界面只能看到这里定义的 [`Snapshot`]。凭据、路径与系统诊断都不在其中。
//!
//! 状态的改动都经过 [`AppState::update`]：改完若与之前不同，就自动推送给界面。
//! 这样既不会漏推，也不会为一次没有变化的改动惊动界面。

use std::sync::{Arc, Mutex, PoisonError};

use assemble_client::Client;
use assemble_protocol::account::Account;
use assemble_protocol::device::Device;
use assemble_protocol::origin::ServerOrigin;
use assemble_store::secret::{SecretKey, SecretStore, SecretString};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;
use tokio_util::sync::CancellationToken;

/// 一次进行中的登录尝试。
#[derive(Debug, Clone)]
pub struct SignIn {
    pub cancel: CancellationToken,
    /// 尝试进行期间一直持有；尝试结束时随记录一起被丢弃。
    _finished: tokio::sync::watch::Sender<bool>,
    /// 等待方用它得知登录流程已经收尾。
    pub finished_signal: tokio::sync::watch::Receiver<bool>,
}

/// 状态变化时发给界面的事件名。
pub const SNAPSHOT_EVENT: &str = "assemble://snapshot";

/// 主窗口的标签。
pub const MAIN_WINDOW: &str = "main";

/// 由登录项启动时附带的参数：这时不弹窗口。
pub const BACKGROUND_ARG: &str = "--background";

/// 连接的是哪一套服务。由构建时定下的服务地址决定，不从界面输入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceTarget {
    Development,
    Production,
}

impl ServiceTarget {
    /// 按地址判断，而不是按调试还是发布：HTTPS 是正式服务，
    /// 回环地址上的 HTTP 是本地开发服务。一个指向本机的发布构建，
    /// 界面上也该如实说它是本地开发服务。
    pub fn for_origin(origin: &ServerOrigin) -> Self {
        if origin.is_secure() {
            Self::Production
        } else {
            Self::Development
        }
    }
}

/// 与服务端之间那条事件流的状态。别的设备在不在线，靠它才知道。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamState {
    /// 没有登录，所以也没有事件流。
    Off,
    Connecting,
    Connected,
    /// 断了，正在自动重连。
    Reconnecting,
}

/// 服务连接的目标，以及它现在能不能连上。显示用的名字由界面决定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceView {
    pub target: ServiceTarget,
    /// 还没探测过时为 `None`。
    pub reachable: Option<bool>,
    pub stream: StreamState,
}

/// 界面上显示的账户信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub id: String,
    pub username: String,
    pub display_name: String,
}

impl From<&Account> for AccountView {
    fn from(account: &Account) -> Self {
        Self {
            id: account.id.clone(),
            username: account.username.clone(),
            display_name: account.display_name.clone(),
        }
    }
}

/// 一台设备在界面上的样子。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    /// `windows`、`macos` 或 `linux`。
    pub platform: String,
    pub online: bool,
}

impl From<&Device> for DeviceView {
    fn from(device: &Device) -> Self {
        Self {
            id: device.id.clone(),
            name: device.name.clone(),
            platform: device.platform.as_str().to_owned(),
            online: device.online,
        }
    }
}

/// 会话所处的阶段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub enum SessionView {
    SignedOut,
    /// 已在系统浏览器打开登录页，等待用户确认。
    SigningIn,
    SignedIn {
        account: AccountView,
        /// 这台安装在本账户里的身份。
        device: DeviceView,
    },
}

impl SessionView {
    /// 这台设备自己的标识；没登录时为 `None`。
    pub fn device_id(&self) -> Option<&str> {
        match self {
            Self::SignedIn { device, .. } => Some(device.id.as_str()),
            Self::SignedOut | Self::SigningIn => None,
        }
    }
}

/// 开机启动在系统里的实际状态。以系统为准，不以点击为准。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LaunchState {
    Enabled,
    Disabled,
    /// 当前运行方式不支持，例如未打包的开发运行。
    Unsupported,
    /// 系统没有给出明确答复，例如 macOS 上等待用户批准。
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoticeKind {
    Info,
    Error,
}

/// 给用户看的一条结果提示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub kind: NoticeKind,
    pub message: String,
}

impl Notice {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            kind: NoticeKind::Info,
            message: message.into(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            kind: NoticeKind::Error,
            message: message.into(),
        }
    }
}

/// 界面在任一时刻能看到的全部状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub service: ServiceView,
    pub session: SessionView,
    /// 账户里的全部设备，包括这台自己，按注册先后排列。没登录时为空。
    pub devices: Vec<DeviceView>,
    pub launch_at_login: LaunchState,
    pub notice: Option<Notice>,
}

/// 读写已保存会话时的失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionStoreError {
    #[error("the system credential store is unavailable")]
    Unavailable,
    #[error("the stored session could not be read")]
    Unreadable,
    #[error("the system credential store rejected the request")]
    Rejected,
}

/// 保存在系统钥匙串里的设备令牌与它对应的公开信息。
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredSession {
    token: String,
    account: Account,
    device: Device,
}

/// 钥匙串里取出来的一整套会话。令牌只以 [`SecretString`] 的形式存在。
#[derive(Debug, Clone)]
pub struct Stored {
    pub token: SecretString,
    pub account: Account,
    pub device: Device,
}

/// 界面能看到的那部分状态。改动都经过 [`AppState::update`]。
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    service: ServiceView,
    session: SessionView,
    devices: Vec<DeviceView>,
    notice: Option<Notice>,
}

impl View {
    /// 这份状态交给界面的样子。`launch_at_login` 由调用方提供：它要问系统，不该在持锁时问。
    fn snapshot(&self, launch_at_login: LaunchState) -> Snapshot {
        Snapshot {
            service: self.service.clone(),
            session: self.session.clone(),
            devices: self.devices.clone(),
            launch_at_login,
            notice: self.notice.clone(),
        }
    }

    fn initial(origin: &ServerOrigin) -> Self {
        Self {
            service: ServiceView {
                target: ServiceTarget::for_origin(origin),
                reachable: None,
                stream: StreamState::Off,
            },
            session: SessionView::SignedOut,
            devices: Vec::new(),
            notice: None,
        }
    }
}

pub struct AppState {
    app: AppHandle,
    pub client: Client,
    /// 系统钥匙串。不可用时为 `None`，此时拒绝登录，而不是把凭据放在别处。
    secrets: Option<Arc<dyn SecretStore>>,
    stored_session_key: SecretKey,
    view: Mutex<View>,
    /// 正在进行的登录尝试；没有时为 `None`。
    sign_in: Mutex<Option<SignIn>>,
    /// 当前这一次登录的后台任务。退出登录或重新登录时取消，换上新的。
    session_cancel: Mutex<CancellationToken>,
    /// 每登录一次加一。后台任务靠它知道该换一条事件流了。
    session_epoch: tokio::sync::watch::Sender<u64>,
    /// 应用退出时触发，用来结束所有后台任务。
    pub lifetime: CancellationToken,
}

impl AppState {
    pub fn new(app: AppHandle, client: Client, secrets: Option<Arc<dyn SecretStore>>) -> Self {
        let origin = client.origin().as_str().to_owned();
        let (session_epoch, _) = tokio::sync::watch::channel(0);
        let view = Mutex::new(View::initial(client.origin()));
        Self {
            app,
            client,
            secrets,
            // 条目名按服务来源区分：换一个服务端就是另一套登录状态。
            stored_session_key: SecretKey::new(
                format!("assemble.device-session:{origin}"),
                "current",
            ),
            view,
            sign_in: Mutex::new(None),
            session_cancel: Mutex::new(CancellationToken::new()),
            session_epoch,
            lifetime: CancellationToken::new(),
        }
    }

    pub fn app(&self) -> &AppHandle {
        &self.app
    }

    /// 能否把登录状态保存到系统钥匙串。
    pub fn can_persist_session(&self) -> bool {
        self.secrets.is_some()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .snapshot(launch_state(&self.app))
    }

    /// 改动状态。与之前不同才推送给界面。
    pub fn update(&self, change: impl FnOnce(&mut View)) {
        // 开机启动的状态要问系统，放到锁外面取：持锁期间不该等任何别的东西。
        let launch_at_login = launch_state(&self.app);
        if let Some(snapshot) = revise(&self.view, change, || launch_at_login) {
            // 没有窗口时什么都不做。
            let _ = self.app.emit(SNAPSHOT_EVENT, snapshot);
        }
    }

    /// 丢掉一条提示。界面已经看过的提示不该一直挂着。
    pub fn dismiss_notice(&self) {
        self.update(|view| view.notice = None);
    }

    pub fn set_reachable(&self, reachable: bool) {
        self.update(|view| view.service.reachable = Some(reachable));
    }

    pub fn set_stream(&self, stream: StreamState) {
        self.update(|view| view.service.stream = stream);
    }

    pub fn set_session(&self, session: SessionView) {
        self.update(|view| view.session = session);
    }

    /// 换上账户里当前的设备列表。
    pub fn set_devices(&self, devices: impl IntoIterator<Item = DeviceView>) {
        self.update(|view| view.devices = devices.into_iter().collect());
    }

    pub fn set_notice(&self, notice: Option<Notice>) {
        self.update(|view| view.notice = notice);
    }

    /// 这台设备自己的标识；没登录时为 `None`。
    pub fn device_id(&self) -> Option<String> {
        self.view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .session
            .device_id()
            .map(ToOwned::to_owned)
    }

    // ---- 登录尝试 ----

    /// 当前正在进行的登录尝试。
    pub fn sign_in_attempt(&self) -> Option<SignIn> {
        self.sign_in
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// 登记一次新的登录尝试；已经有尝试在进行时返回 `None`。
    pub fn begin_sign_in(&self) -> Option<(CancellationToken, tokio::sync::watch::Receiver<bool>)> {
        let mut sign_in = self.sign_in.lock().unwrap_or_else(PoisonError::into_inner);
        if sign_in.is_some() {
            return None;
        }
        let cancel = self.lifetime.child_token();
        let (finished, receiver) = tokio::sync::watch::channel(false);
        *sign_in = Some(SignIn {
            cancel: cancel.clone(),
            _finished: finished,
            finished_signal: receiver.clone(),
        });
        Some((cancel, receiver))
    }

    /// 结束一次登录尝试。
    pub fn end_sign_in(&self, attempt: &CancellationToken) {
        let mut sign_in = self.sign_in.lock().unwrap_or_else(PoisonError::into_inner);
        if sign_in
            .as_ref()
            .is_some_and(|sign_in| sign_in.cancel == *attempt)
        {
            *sign_in = None;
        }
    }

    // ---- 登录会话 ----

    /// 开始一次新的登录会话：旧会话的后台任务到此为止，并通知后台任务重新开始。
    pub fn begin_session(&self) {
        *self
            .session_cancel
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = self.lifetime.child_token();
        self.session_epoch
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    /// 当前登录会话的取消令牌。只在登录期间有效，退出或重新登录时触发。
    pub fn session_cancel(&self) -> CancellationToken {
        self.session_cancel
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// 登录会话的更替通知。每次登录之后都会变一次。
    pub fn session_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.session_epoch.subscribe()
    }

    /// 结束登录会话：后台任务停下来，与账户有关的状态清空。
    pub fn end_session(&self) {
        self.session_cancel
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cancel();
        self.session_epoch
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        self.set_devices(Vec::new());
        self.set_stream(StreamState::Off);
    }

    // ---- 钥匙串 ----

    /// 取出保存过的会话；没有保存过时为 `None`。
    pub fn stored_session(&self) -> Result<Option<Stored>, SessionStoreError> {
        let Some(secrets) = &self.secrets else {
            return Err(SessionStoreError::Unavailable);
        };
        let Some(secret) = secrets
            .get(&self.stored_session_key)
            .map_err(|_| SessionStoreError::Rejected)?
        else {
            return Ok(None);
        };
        let document: StoredSession =
            serde_json::from_str(secret.expose()).map_err(|_| SessionStoreError::Unreadable)?;
        Ok(Some(Stored {
            token: SecretString::new(document.token),
            account: document.account,
            device: document.device,
        }))
    }

    pub fn store_session(&self, session: &Stored) -> Result<(), SessionStoreError> {
        let secrets = self
            .secrets
            .as_ref()
            .ok_or(SessionStoreError::Unavailable)?;
        let document = StoredSession {
            token: session.token.expose().to_owned(),
            account: session.account.clone(),
            device: session.device.clone(),
        };
        let encoded = SecretString::new(
            serde_json::to_string(&document).map_err(|_| SessionStoreError::Rejected)?,
        );
        secrets
            .put(&self.stored_session_key, &encoded)
            .map_err(|_| SessionStoreError::Rejected)
    }

    pub fn clear_stored_session(&self) -> Result<(), SessionStoreError> {
        let secrets = self
            .secrets
            .as_ref()
            .ok_or(SessionStoreError::Unavailable)?;
        secrets
            .delete(&self.stored_session_key)
            .map_err(|_| SessionStoreError::Rejected)
    }
}

/// 改动状态，返回改了什么；没有变化时为 `None`。
///
/// 快照必须在锁内算好：这个函数返回时锁已经放开，之后再读状态就是另一次加锁了。
/// 曾经在这里犯过一个错——持着锁去调一个会再次加锁的函数，线程把自己锁死，
/// 而且永远占着这把锁。所以"比较"与"快照"都收在这一个函数里，不给出在锁内回调的机会。
fn revise(
    view: &Mutex<View>,
    change: impl FnOnce(&mut View),
    launch_at_login: impl FnOnce() -> LaunchState,
) -> Option<Snapshot> {
    let mut view = view.lock().unwrap_or_else(PoisonError::into_inner);
    let before = view.clone();
    change(&mut view);
    (*view != before).then(|| view.snapshot(launch_at_login()))
}

/// 开机启动在系统里的实际状态。
pub fn launch_state(app: &AppHandle) -> LaunchState {
    if !launch_at_login_supported() {
        return LaunchState::Unsupported;
    }
    match app.autolaunch().is_enabled() {
        Ok(true) => LaunchState::Enabled,
        Ok(false) => LaunchState::Disabled,
        // macOS 上等待用户批准时读不到明确状态；这不代表已启用。
        Err(_) => LaunchState::Unknown,
    }
}

/// 只有打包后的正式运行才登记开机启动；开发运行不写系统登录项。
pub fn launch_at_login_supported() -> bool {
    !cfg!(debug_assertions)
}

/// 显示主窗口：还原、显示并聚焦。
pub fn show_main(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        tracing::warn!("the main window is missing");
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

#[cfg(test)]
mod tests {
    use assemble_protocol::primitives::{Platform, Timestamp};

    use super::*;

    /// 与 TypeScript 侧共用的状态样本：`apps/desktop/contract/snapshots.json`。
    const SAMPLES: &str = include_str!("../../contract/snapshots.json");

    fn device(id: &str, name: &str, online: bool) -> DeviceView {
        DeviceView {
            id: id.into(),
            name: name.into(),
            platform: "macos".into(),
            online,
        }
    }

    #[test]
    fn every_contract_sample_round_trips_through_the_snapshot_type() {
        let expected: serde_json::Value = serde_json::from_str(SAMPLES).unwrap();
        let snapshots: Vec<Snapshot> = serde_json::from_value(expected.clone()).unwrap();
        let actual = serde_json::to_value(&snapshots).unwrap();
        assert_eq!(
            actual, expected,
            "the shell and the interface disagree about the state shape"
        );
    }

    #[test]
    fn the_snapshot_never_carries_credentials() {
        for forbidden in ["token", "Bearer", "password", "cookie", "/Users/"] {
            assert!(
                !SAMPLES.contains(forbidden),
                "{forbidden} must not appear in samples"
            );
        }
    }

    #[test]
    fn only_a_signed_in_session_has_a_device_id() {
        assert_eq!(SessionView::SignedOut.device_id(), None);
        assert_eq!(SessionView::SigningIn.device_id(), None);
        let signed_in = SessionView::SignedIn {
            account: AccountView {
                id: "a1".into(),
                username: "example".into(),
                display_name: "Example".into(),
            },
            device: device("d1", "Mac", true),
        };
        assert_eq!(signed_in.device_id(), Some("d1"));
    }

    #[test]
    fn the_initial_view_is_signed_out_with_nothing_else() {
        let local = ServerOrigin::development();
        let view = View::initial(&local);
        assert_eq!(view.session, SessionView::SignedOut);
        assert!(view.devices.is_empty() && view.notice.is_none());
        assert_eq!(view.service.stream, StreamState::Off);
        assert_eq!(view.service.reachable, None);
        assert_eq!(view.service.target, ServiceTarget::Development);
        let production =
            ServerOrigin::parse("https://assemble.example").expect("a valid production origin");
        assert_eq!(
            View::initial(&production).service.target,
            ServiceTarget::Production
        );
        // 开机启动只在打包后的正式运行里提供；测试运行在调试构建里。
        assert!(!launch_at_login_supported());
    }

    #[test]
    fn revising_returns_a_snapshot_only_when_something_changed() {
        let view = Mutex::new(View::initial(&ServerOrigin::development()));
        let launch = || LaunchState::Disabled;

        // 没有变化：不惊动界面。
        assert!(revise(&view, |_| {}, launch).is_none());
        assert!(revise(&view, |v| v.notice = None, launch).is_none());

        let snapshot = revise(&view, |v| v.notice = Some(Notice::info("你好")), launch).unwrap();
        assert_eq!(snapshot.notice, Some(Notice::info("你好")));
        assert_eq!(snapshot.launch_at_login, LaunchState::Disabled);
        assert_eq!(snapshot.service.stream, StreamState::Off);
        // 改完之后锁是放开的：同一个线程马上还能再读、再改。
        let snapshot = revise(&view, |v| v.devices.push(device("d1", "Mac", true)), launch)
            .expect("the device list changed");
        assert_eq!(snapshot.devices.len(), 1);
        assert_eq!(
            view.lock()
                .unwrap()
                .snapshot(LaunchState::Enabled)
                .devices
                .len(),
            1
        );
        // 换回原值也算一次变化：界面得看到它被撤回了。
        assert!(revise(&view, |v| v.notice = None, launch).is_some());
    }

    #[test]
    fn device_views_carry_the_online_flag_and_the_servers_platform_spelling() {
        let device = Device {
            id: "d1".into(),
            name: "工作电脑".into(),
            platform: Platform::Windows,
            online: true,
            last_seen_at: None,
            created_at: Timestamp::now(),
        };
        let view = DeviceView::from(&device);
        assert!(view.online);
        assert_eq!(view.platform, "windows");
        assert_eq!(serde_json::to_value(&view).unwrap()["platform"], "windows");
    }
}
