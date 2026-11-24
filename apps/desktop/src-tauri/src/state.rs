//! 应用状态：服务是否可达、会话处在哪个阶段、有哪些设备，
//! 以及这些状态如何交给界面。收发的记录在 [`crate::registry`]。
//!
//! 界面只能看到这里定义的 [`Snapshot`]。凭据、路径与系统诊断都不在其中：
//! 文件在本机的哪里，只记在 Rust 里，界面拿到的是它的名字。
//!
//! 状态的改动都经过 [`AppState::update`]：改完若与之前不同，就自动推送给界面。
//! 这样既不会漏推，也不会为一次没有变化的改动惊动界面。

use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use assemble_client::{Client, OutgoingFile};
use assemble_device::inbox::ReceiveFailure;
use assemble_protocol::account::Account;
use assemble_protocol::device::Device;
use assemble_protocol::drive::{DriveFile, DriveListing};
use assemble_protocol::origin::ServerOrigin;
use assemble_protocol::primitives::Timestamp;
use assemble_protocol::sends::{Direction, Send as SendRecord};
use assemble_store::secret::{SecretKey, SecretStore, SecretString};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;
use tokio_util::sync::CancellationToken;

use crate::registry::{
    Cancellation, Delivery, Destination, DraftView, Next, Outgoing, Registry, TransferFailure,
    TransferView, Updated,
};

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

/// 收到的文件存在哪里。界面只知道是在默认位置还是用户选的文件夹，
/// 以及那个文件夹叫什么；完整的位置不经过界面。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveView {
    /// 用户选的文件夹的名字；用默认位置（下载/Assemble）时为 `None`。
    pub custom_folder: Option<String>,
}

impl ReceiveView {
    /// 一个接收文件夹在界面上的样子：只说得出它的名字。
    fn for_folder(folder: Option<&std::path::Path>) -> Self {
        Self {
            custom_folder: folder
                .and_then(std::path::Path::file_name)
                .map(|name| name.to_string_lossy().into_owned()),
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
    /// 最近的传输，新的在前。没登录时为空。
    pub transfers: Vec<TransferView>,
    /// 用户挑好、还没决定发给谁的文件。没有草稿时为 `None`。
    pub draft: Option<DraftView>,
    /// 云盘里的文件与空间。没登录时为 `None`。
    pub drive: Option<DriveView>,
    pub receive: ReceiveView,
    pub launch_at_login: LaunchState,
    pub notice: Option<Notice>,
}

/// 云盘里的一个文件在界面上的样子。摘要不给界面：用不上。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveFileView {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub created_at: Timestamp,
}

/// 云盘在界面上的样子：有哪些文件，空间用了多少。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveView {
    /// 新的在前。
    pub items: Vec<DriveFileView>,
    pub used_bytes: u64,
    pub limit_bytes: u64,
}

impl From<&DriveListing> for DriveView {
    fn from(listing: &DriveListing) -> Self {
        Self {
            items: listing
                .items
                .iter()
                .map(|file| DriveFileView {
                    id: file.id.clone(),
                    name: file.name.clone(),
                    size: file.size,
                    created_at: file.created_at,
                })
                .collect(),
            used_bytes: listing.used_bytes,
            limit_bytes: listing.limit_bytes,
        }
    }
}

/// 文件收到之后怎么告诉用户。桌面壳用系统通知；测试里用一个不弹窗的实现，
/// 免得跑一次测试就在屏幕上冒出一条通知。
pub trait Notifier: Send + Sync {
    /// `handle` 是这条通知的标识：用户点它时，按它找回要显示的文件。
    fn received(&self, handle: i32, peer: &str, file_name: &str, fell_back: bool);
}

/// 本机的设置，保存在应用数据目录里。不随账户变化：换个人登录，文件还是存到同一个地方。
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// 用户选的接收文件夹（规范路径）；`None` 表示默认位置。
    pub receive_directory: Option<PathBuf>,
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
    /// 收发的记录与待发送的草稿。它同时是后台任务的队列。
    registry: Registry,
    /// 云盘最近一次的名单；拿不到时留着上一份。
    drive: Option<DriveListing>,
    receive: ReceiveView,
    notice: Option<Notice>,
}

impl View {
    fn with_receive_folder(mut self, folder: Option<&std::path::Path>) -> Self {
        self.receive = ReceiveView::for_folder(folder);
        self
    }

    /// 这份状态交给界面的样子。`launch_at_login` 由调用方提供：它要问系统，不该在持锁时问。
    fn snapshot(&self, launch_at_login: LaunchState) -> Snapshot {
        Snapshot {
            service: self.service.clone(),
            session: self.session.clone(),
            devices: self.devices.clone(),
            transfers: self.registry.views(),
            draft: self.registry.draft_view(),
            drive: self.drive.as_ref().map(DriveView::from),
            receive: self.receive.clone(),
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
            registry: Registry::default(),
            drive: None,
            receive: ReceiveView {
                custom_folder: None,
            },
            notice: None,
        }
    }
}

pub struct AppState<R: tauri::Runtime = tauri::Wry> {
    app: AppHandle<R>,
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
    /// 正在接收的文件各自的取消令牌：对方撤回时用来停下那一次接收。
    active: Mutex<Vec<(String, CancellationToken)>>,
    /// 传输记录一有变化就叫醒接收任务。
    transfers_changed: tokio::sync::Notify,
    /// 下一条系统通知的标识。
    next_handle: AtomicI32,
    settings: Mutex<Settings>,
    /// 系统的"下载"目录：默认的接收位置在它之下。
    downloads: PathBuf,
    notifier: Arc<dyn Notifier>,
}

impl<R: tauri::Runtime> AppState<R> {
    pub fn new(
        app: AppHandle<R>,
        client: Client,
        secrets: Option<Arc<dyn SecretStore>>,
        settings: Settings,
        downloads: PathBuf,
        notifier: Arc<dyn Notifier>,
    ) -> Self {
        let origin = client.origin().as_str().to_owned();
        let (session_epoch, _) = tokio::sync::watch::channel(0);
        let view = Mutex::new(
            View::initial(client.origin())
                .with_receive_folder(settings.receive_directory.as_deref()),
        );
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
            active: Mutex::new(Vec::new()),
            transfers_changed: tokio::sync::Notify::new(),
            next_handle: AtomicI32::new(1),
            settings: Mutex::new(settings),
            downloads,
            notifier,
        }
    }

    pub fn app(&self) -> &AppHandle<R> {
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
        let ((), snapshot) = revise(&self.view, change, || launch_at_login);
        // 没有窗口时什么都不做。
        if let Some(snapshot) = snapshot {
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

    /// 账户里的一台设备。
    pub fn device(&self, id: &str) -> Option<DeviceView> {
        self.view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .devices
            .iter()
            .find(|device| device.id == id)
            .cloned()
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
        // 传输记录属于那个账户：文件留在接收目录里，但名单不留给下一个登录的人。
        self.set_devices(Vec::new());
        self.registry(Registry::clear);
        self.set_drive(None);
        self.set_stream(StreamState::Off);
    }

    /// 系统的"下载"目录。默认的接收位置是它之下的 `Assemble`。
    pub fn downloads_dir(&self) -> &std::path::Path {
        &self.downloads
    }

    /// 本机的设置。
    pub fn settings(&self) -> Settings {
        self.settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// 换一个接收文件夹；传 `None` 回到默认位置。
    pub fn set_receive_folder(&self, folder: Option<&std::path::Path>) {
        self.settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .receive_directory = folder.map(ToOwned::to_owned);
        self.update(|view| view.receive = ReceiveView::for_folder(folder));
    }

    /// 改动传输记录表；有变化才推送给界面。返回改动函数的结果。
    fn registry<T>(&self, change: impl FnOnce(&mut Registry) -> T) -> T {
        // 开机启动的状态要问系统，放到锁外面取：持锁期间不该等任何别的东西。
        let launch_at_login = launch_state(&self.app);
        let (result, snapshot) = revise(
            &self.view,
            |view| change(&mut view.registry),
            || launch_at_login,
        );
        // 没有窗口时什么都不做。
        if let Some(snapshot) = snapshot {
            let _ = self.app.emit(SNAPSHOT_EVENT, snapshot);
        }
        result
    }

    // ---- 传输 ----

    /// 别的设备发来了一个文件，或者服务端还在等这台设备处理它。
    pub fn offer(&self, send: &SendRecord) {
        self.registry(|registry| registry.offer(send));
        self.transfers_changed.notify_one();
    }

    /// 服务端说一次发送变了。返回 `true` 表示文件其实到了服务端，而本机已经放弃了：
    /// 调用方要去服务端撤回。
    pub fn updated(&self, send: &SendRecord) -> bool {
        // 收到的文件还在接收途中时对面撤回了：那一次接收到此为止。
        if send.direction == Direction::In && send.state.is_terminal() {
            self.stop_transfer(&send.id);
        }
        self.registry(|registry| registry.updated(send)) == Updated::Withdraw
    }

    /// 服务端还在等着交出去的这次发送，是不是本机已经放弃的那一个。
    pub fn abandoned(&self, send: &SendRecord) -> bool {
        self.view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .registry
            .abandoned(send)
    }

    /// 服务端已经不认这条记录了（列表里找不到）：本机也不必再留着它。
    pub fn forget_transfer(&self, id: &str) {
        self.registry(|registry| registry.forget(id));
    }

    /// 后台任务该做的下一件事。
    pub fn next_job(&self) -> Next {
        self.registry(|registry| registry.next_job(Instant::now()))
    }

    /// 等待传输记录发生变化。
    pub async fn wait_for_transfers(&self) {
        self.transfers_changed.notified().await;
    }

    /// 领取一个排着队的文件去收：别的设备发来的，或者云盘里的。
    pub fn begin_receive(&self, id: &str) -> Option<Delivery> {
        self.registry(|registry| registry.begin_receive(id))
    }

    /// 领取一个排着队的文件去发。
    pub fn begin_send(&self, id: &str) -> Option<Outgoing> {
        self.registry(|registry| registry.begin_send(id))
    }

    /// 这一次传输可以被打断：取消时按标识找到它。
    pub fn track_transfer(&self, id: &str, cancel: CancellationToken) -> TrackedTransfer<'_, R> {
        self.active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((id.to_owned(), cancel.clone()));
        TrackedTransfer {
            state: self,
            id: id.to_owned(),
        }
    }

    /// 停下正在进行的那一次传输。没在进行时什么都不做。
    pub fn stop_transfer(&self, id: &str) {
        let mut active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        let (stopped, untouched): (Vec<_>, Vec<_>) = std::mem::take(&mut *active)
            .into_iter()
            .partition(|(tracked, _)| tracked == id);
        *active = untouched;
        drop(active);
        for (_, cancel) in stopped {
            cancel.cancel();
        }
    }

    /// 传输进度。
    pub fn transfer_progress(&self, id: &str, fraction: f64) {
        self.registry(|registry| registry.progress(id, fraction));
    }

    /// 收到的文件已经存好，回执还没发。
    pub fn receive_completed(&self, id: &str, name: &str, path: PathBuf, handle: i32) {
        self.registry(|registry| registry.receive_completed(id, name, path, handle));
        self.transfers_changed.notify_one();
    }

    /// 收不下来。回执还没发。
    pub fn receive_failed(&self, id: &str, failure: TransferFailure) {
        self.registry(|registry| registry.receive_failed(id, failure));
        self.transfers_changed.notify_one();
    }

    /// 这次没成，过一会儿再从头收一遍。
    pub fn retry_receive(&self, id: &str) {
        self.registry(|registry| registry.retry_receive(id, Instant::now()));
    }

    /// 回执没发出去，过一会儿再补。
    pub fn retry_settle(&self, id: &str) {
        self.registry(|registry| registry.retry_settle(id, Instant::now()));
    }

    /// 回执发出去了，或者服务端本来就知道结局。
    pub fn transfer_settled(&self, id: &str) {
        self.registry(|registry| registry.settled(id));
    }

    /// 需要补发回执的记录。
    pub fn settlement_of(&self, id: &str) -> Option<(SendRecord, Option<ReceiveFailure>)> {
        self.view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .registry
            .settlement_of(id)
    }

    /// 一个已经收到的文件在本机的位置。
    pub fn saved_file(&self, id: &str) -> Option<PathBuf> {
        self.view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .registry
            .saved_file(id)
    }

    /// 用户点了某条通知：它说的那个文件在哪。
    pub fn saved_file_for_handle(&self, handle: i32) -> Option<PathBuf> {
        self.view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .registry
            .saved_file_for_handle(handle)
    }

    // ---- 发出的 ----

    // ---- 云盘 ----

    /// 换上服务端给的云盘名单。
    pub fn set_drive(&self, listing: Option<DriveListing>) {
        self.update(|view| view.drive = listing);
    }

    /// 云盘名单上的一个文件；名单上没有时为 `None`。
    pub fn drive_file(&self, id: &str) -> Option<DriveFile> {
        self.view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drive
            .as_ref()?
            .items
            .iter()
            .find(|file| file.id == id)
            .cloned()
    }

    /// 把挑好的文件发往某处（不经过草稿）。返回每一条记录的标识。
    pub fn queue_files(&self, destination: Destination, files: Vec<OutgoingFile>) -> Vec<String> {
        let ids = self.registry(|registry| registry.queue(destination, files));
        self.transfers_changed.notify_one();
        ids
    }

    /// 用户挑好了要发的文件。
    pub fn set_draft(&self, files: Vec<OutgoingFile>) {
        self.registry(|registry| registry.set_draft(files));
    }

    /// 发送面板关掉了：挑好的文件不再留着。
    pub fn drop_draft(&self) {
        self.registry(Registry::drop_draft);
    }

    /// 把草稿里的文件发往某处：一台设备，或者云盘。返回每一条记录的标识。
    pub fn queue_draft(&self, destination: Destination) -> Vec<String> {
        let ids = self.registry(|registry| registry.queue_draft(destination));
        self.transfers_changed.notify_one();
        ids
    }

    /// 排队去云盘里取一个文件回来。
    pub fn fetch_from_drive(&self, file: &DriveFile) -> String {
        let id = self.registry(|registry| registry.fetch_from_drive(file));
        self.transfers_changed.notify_one();
        id
    }

    /// 文件存进云盘了。
    pub fn drive_uploaded(&self, id: &str) -> bool {
        let stored = self.registry(|registry| registry.drive_uploaded(id));
        self.transfers_changed.notify_one();
        stored
    }

    /// 服务端收下了文件，正在等对方接收。返回 `true` 表示文件得去服务端撤回。
    pub fn send_uploaded(&self, id: &str, send: &SendRecord) -> bool {
        self.registry(|registry| registry.send_uploaded(id, send)) == Updated::Withdraw
    }

    pub fn send_failed(&self, id: &str, failure: TransferFailure) {
        self.registry(|registry| registry.send_failed(id, failure));
    }

    /// 网络断了，过一会儿再从头发一遍。
    pub fn retry_send(&self, id: &str) {
        self.registry(|registry| registry.retry_send(id, Instant::now()));
    }

    /// 再发一次。只有"文件根本没到服务端"的失败才能重发。
    pub fn retry_failed_send(&self, id: &str) -> bool {
        let retried = self.registry(|registry| registry.retry_failed_send(id, Instant::now()));
        self.transfers_changed.notify_one();
        retried
    }

    /// 用户要取消一条传输。
    pub fn cancel_transfer(&self, id: &str) -> Cancellation {
        let outcome = self.registry(|registry| registry.cancel(id));
        self.transfers_changed.notify_one();
        outcome
    }

    /// 给下一条系统通知用的标识。用完就加一，不会与还在通知中心里的那条重复。
    pub fn fresh_handle(&self) -> i32 {
        self.next_handle.fetch_add(1, Ordering::Relaxed)
    }

    /// 文件收到了，告诉用户。文件这时已经登记好，点通知时找得回它。
    pub fn notify_received(&self, handle: i32, peer: &str, file_name: &str, fell_back: bool) {
        self.notifier.received(handle, peer, file_name, fell_back);
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

/// 一个正在进行的传输：离开作用域时就不再跟踪它。
pub struct TrackedTransfer<'a, R: tauri::Runtime> {
    state: &'a AppState<R>,
    id: String,
}

impl<R: tauri::Runtime> Drop for TrackedTransfer<'_, R> {
    fn drop(&mut self) {
        self.state
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(tracked, _)| tracked != &self.id);
    }
}

/// 改动状态：返回改动函数的结果，以及改了什么；没有变化时后者为 `None`。
///
/// 快照必须在锁内算好：这个函数返回时锁已经放开，之后再读状态就是另一次加锁了。
/// 曾经在这里犯过一个错——持着锁去调一个会再次加锁的函数，线程把自己锁死，
/// 而且永远占着这把锁。所以"比较"与"快照"都收在这一个函数里，不给出在锁内回调的机会。
fn revise<T>(
    view: &Mutex<View>,
    change: impl FnOnce(&mut View) -> T,
    launch_at_login: impl FnOnce() -> LaunchState,
) -> (T, Option<Snapshot>) {
    let mut view = view.lock().unwrap_or_else(PoisonError::into_inner);
    let before = view.clone();
    let result = change(&mut view);
    (
        result,
        (*view != before).then(|| view.snapshot(launch_at_login())),
    )
}

/// 开机启动在系统里的实际状态。
pub fn launch_state<R: tauri::Runtime>(app: &AppHandle<R>) -> LaunchState {
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
pub fn show_main<R: tauri::Runtime>(app: &AppHandle<R>) {
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
        assert!(revise(&view, |_| {}, launch).1.is_none());
        assert!(revise(&view, |v| v.notice = None, launch).1.is_none());

        // 改动函数的结果原样交回，快照在它之后。
        let (devices, snapshot) = revise(&view, |v| v.devices.len(), launch);
        assert_eq!(devices, 0);
        assert!(snapshot.is_none());

        let ((), snapshot) = revise(&view, |v| v.notice = Some(Notice::info("你好")), launch);
        let snapshot = snapshot.unwrap();
        assert_eq!(snapshot.notice, Some(Notice::info("你好")));
        assert_eq!(snapshot.launch_at_login, LaunchState::Disabled);
        assert_eq!(snapshot.service.stream, StreamState::Off);
        // 改完之后锁是放开的：同一个线程马上还能再读、再改。
        let ((), snapshot) = revise(&view, |v| v.devices.push(device("d1", "Mac", true)), launch);
        let snapshot = snapshot.expect("the device list changed");
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
        assert!(revise(&view, |v| v.notice = None, launch).1.is_some());
    }

    #[test]
    fn the_interface_only_learns_a_receive_folders_name() {
        use std::path::Path;

        assert_eq!(ReceiveView::for_folder(None).custom_folder, None);
        assert_eq!(
            ReceiveView::for_folder(Some(Path::new("/Users/example/工作资料"))).custom_folder,
            Some("工作资料".to_owned())
        );
        // 选到磁盘根目录时说不出名字，而不是把一串分隔符交给界面。
        assert_eq!(
            ReceiveView::for_folder(Some(Path::new("/"))).custom_folder,
            None
        );
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
