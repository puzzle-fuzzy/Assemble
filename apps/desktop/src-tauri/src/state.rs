//! 应用状态：服务是否可达、会话处在哪个阶段、有哪些设备、有哪些传输，
//! 以及这些状态如何交给界面。
//!
//! 界面只能看到这里定义的 [`Snapshot`]。凭据、路径与系统诊断都不在其中：
//! 收到的文件存在本机的哪里，只记在 [`TransferRecord`] 里，界面拿到的是它的名字。
//!
//! 状态的改动都经过 [`AppState::update`]：改完若与之前不同，就自动推送给界面。
//! 这样既不会漏推，也不会为一次没有变化的改动惊动界面。

use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use assemble_client::Client;
use assemble_device::Received;
use assemble_device::inbox::ReceiveFailure;
use assemble_protocol::account::Account;
use assemble_protocol::device::Device;
use assemble_protocol::origin::ServerOrigin;
use assemble_protocol::sends::{Direction, Send as SendRecord, SendState};
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

/// 一次传输走到了哪一步。收与发共用一套说法，界面按方向各自措辞。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferPhase {
    /// 排着队还没开始，或者上一次没成功、等着再试。
    Queued,
    InProgress,
    /// 收到的文件已经存好；发出去的文件对方已经收下。
    Complete,
    Failed,
    /// 发送方撤回了。
    Cancelled,
    /// 过了时限还没送达。
    Expired,
}

/// 传输为什么失败。与协议里报告给对端的原因一一对应，由下面的转换负责；
/// 状态形状一律用驼峰拼写，所以这里的拼法与服务端那边的不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferFailure {
    DiskFull,
    WriteFailed,
    VerificationFailed,
    Cancelled,
}

impl From<ReceiveFailure> for TransferFailure {
    fn from(failure: ReceiveFailure) -> Self {
        match failure {
            ReceiveFailure::DiskFull => Self::DiskFull,
            ReceiveFailure::WriteFailed => Self::WriteFailed,
            ReceiveFailure::VerificationFailed => Self::VerificationFailed,
            ReceiveFailure::Cancelled => Self::Cancelled,
        }
    }
}

impl From<TransferFailure> for ReceiveFailure {
    fn from(failure: TransferFailure) -> Self {
        match failure {
            TransferFailure::DiskFull => Self::DiskFull,
            TransferFailure::WriteFailed => Self::WriteFailed,
            TransferFailure::VerificationFailed => Self::VerificationFailed,
            TransferFailure::Cancelled => Self::Cancelled,
        }
    }
}

/// 一次传输在界面上的样子。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferView {
    /// 服务端那一次发送的标识。界面拿它引用这条传输，别的什么都传不了。
    pub id: String,
    /// `in` 是别的设备发来的，`out` 是这台设备发出去的。
    pub direction: Direction,
    /// 对端设备的名字，是传输当时的。
    pub peer: String,
    pub name: String,
    pub size: u64,
    pub phase: TransferPhase,
    /// 0 到 1。只在 [`TransferPhase::InProgress`] 期间变化。
    pub progress: f64,
    /// 实际保存下来的文件名；与原名字不同时才有。
    pub saved_as: Option<String>,
    pub failure: Option<TransferFailure>,
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
    pub receive: ReceiveView,
    pub launch_at_login: LaunchState,
    pub notice: Option<Notice>,
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

/// 一条传输记录：界面看到的 [`TransferView`]，加上只有本机用得上的那部分。
#[derive(Debug, Clone, PartialEq)]
pub struct TransferRecord {
    pub view: TransferView,
    /// 服务端那次发送的记录：重新接收时要核对它的长度与摘要。
    pub send: SendRecord,
    /// 已经试了几次。只用来决定下一次隔多久再试。
    pub attempts: u32,
    /// 下一次可以动手的时间；`None` 表示随时可以。
    pub not_before: Option<Instant>,
    /// 服务端已经知道结局：回执确认过了，或者结局本来就是服务端说的。
    pub settled: bool,
    /// 文件在本机的位置，用来"在文件夹中显示"。不交给界面。
    pub saved_path: Option<PathBuf>,
    /// 通知的系统标识：点通知时据此找到要显示的文件。
    pub handle: Option<i32>,
}

impl TransferRecord {
    /// 为一次别的设备发来的文件排队。
    fn queued(send: &SendRecord) -> Self {
        Self {
            view: TransferView {
                id: send.id.clone(),
                direction: send.direction,
                peer: send.peer_device_name.clone(),
                name: send.name.clone(),
                size: send.size,
                phase: TransferPhase::Queued,
                progress: 0.0,
                saved_as: None,
                failure: None,
            },
            send: send.clone(),
            attempts: 0,
            not_before: None,
            settled: false,
            saved_path: None,
            handle: None,
        }
    }

    fn id(&self) -> &str {
        &self.view.id
    }
}

/// 接收任务该做的下一件事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// 把一个排着队的文件收下来。
    Receive { id: String },
    /// 把本机已经有的结局告诉服务端：收下了，或者收不下来。
    Settle { id: String },
}

/// 接收任务现在有没有事可做。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    Nothing,
    /// 现在没有，但这么多时间之后会有。
    Wait(Duration),
    Work(Job),
}

/// 传输记录最多留这么多条；超了就丢掉最旧的已经结束的记录。
const MAX_TRANSFERS: usize = 100;

/// 回执最多补发这么多次。再多次服务端都不收，就只能等它自己过期。
const SETTLE_ATTEMPTS: u32 = 8;

/// 每多试一次等多久：2 秒、5 秒、15 秒、1 分钟，之后一直是 5 分钟。
fn backoff(attempts: u32) -> Duration {
    const STEPS: [Duration; 5] = [
        Duration::from_secs(2),
        Duration::from_secs(5),
        Duration::from_secs(15),
        Duration::from_secs(60),
        Duration::from_secs(300),
    ];
    STEPS[usize::try_from(attempts.saturating_sub(1))
        .unwrap_or(0)
        .min(STEPS.len() - 1)]
}

/// 从一堆记录里挑出该做的下一件事。接收记录是新的在前，这里倒着看：先来后到。
///
/// 先收文件，再补回执：文件早点到用户手里比回执要紧。
fn pick(records: &[TransferRecord], now: Instant) -> Next {
    let wait_of = |record: &TransferRecord| {
        record
            .not_before
            .map_or(Duration::ZERO, |time| time.saturating_duration_since(now))
    };
    let mut soonest = Duration::MAX;
    for record in records.iter().rev() {
        if record.view.phase == TransferPhase::Queued {
            let wait = wait_of(record);
            if wait.is_zero() {
                return Next::Work(Job::Receive {
                    id: record.view.id.clone(),
                });
            }
            soonest = soonest.min(wait);
        }
    }
    for record in records.iter().rev() {
        let unfinished = matches!(
            record.view.phase,
            TransferPhase::Complete | TransferPhase::Failed
        ) && !record.settled;
        if unfinished {
            let wait = wait_of(record);
            if wait.is_zero() {
                return Next::Work(Job::Settle {
                    id: record.view.id.clone(),
                });
            }
            soonest = soonest.min(wait);
        }
    }
    match soonest {
        Duration::MAX => Next::Nothing,
        wait => Next::Wait(wait),
    }
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
    /// 最近的传输，新的在前。它同时是接收任务的队列。
    transfers: Vec<TransferRecord>,
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
            transfers: self
                .transfers
                .iter()
                .map(|record| record.view.clone())
                .collect(),
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
            transfers: Vec::new(),
            receive: ReceiveView {
                custom_folder: None,
            },
            notice: None,
        }
    }

    /// 找到一条传输记录。
    fn record(&mut self, id: &str) -> Option<&mut TransferRecord> {
        self.transfers.iter_mut().find(|record| record.id() == id)
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
        // 传输记录属于那个账户：文件留在接收目录里，但名单不留给下一个登录的人。
        self.set_devices(Vec::new());
        self.set_transfers(Vec::new());
        self.set_stream(StreamState::Off);
    }

    pub fn set_transfers(&self, transfers: Vec<TransferRecord>) {
        self.update(|view| view.transfers = transfers);
        self.transfers_changed.notify_one();
    }

    // ---- 接收位置 ----

    /// 系统的"下载"目录。默认的接收位置是它之下的 `Assemble`。
    pub fn downloads_dir(&self) -> &std::path::Path {
        &self.downloads
    }

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

    // ---- 传输 ----

    /// 别的设备发来了一个文件，或者服务端还在等这台设备处理它。
    /// 按本机已经走到哪一步接上，不会把同一个文件收两遍。
    pub fn offer(&self, send: &SendRecord) {
        // 只处理"发给我的、还在等着"的记录；别的情况由 [`AppState::updated`] 负责。
        if send.state != SendState::Offered || send.direction != Direction::In {
            return;
        }
        self.update(|view| {
            if let Some(record) = view.record(&send.id) {
                record.catch_up(send);
            } else {
                view.transfers.insert(0, TransferRecord::queued(send));
                // 太多了就丢掉最旧的、已经结束的记录。
                if view.transfers.len() > MAX_TRANSFERS
                    && let Some(index) = view
                        .transfers
                        .iter()
                        .rposition(|record| record.view.phase.is_finished())
                {
                    view.transfers.remove(index);
                }
            }
        });
        self.transfers_changed.notify_one();
    }

    /// 服务端说一次发送变了：对面撤回了、过期了，或者本机的回执被确认了。
    pub fn updated(&self, send: &SendRecord) {
        // 文件还在接收途中时对面撤回了：那一次接收到此为止。
        if send.direction == Direction::In && send.state.is_terminal() {
            self.stop_receive(&send.id);
        }
        self.update(|view| {
            let Some(record) = view.record(&send.id) else {
                return;
            };
            let withdrawn = matches!(send.state, SendState::Cancelled | SendState::Expired);
            // 已经存到本机的文件以本机为准：对面这时候撤回，也撤不回已经落盘的文件。
            let already_saved = matches!(
                record.view.phase,
                TransferPhase::Complete | TransferPhase::Failed
            );
            if withdrawn && !already_saved {
                record.view.phase = if send.state == SendState::Cancelled {
                    TransferPhase::Cancelled
                } else {
                    TransferPhase::Expired
                };
                record.view.progress = 0.0;
                record.settled = true;
                record.not_before = None;
            } else if send.state == SendState::Delivered && send.direction == Direction::In {
                // 收到的文件有了结局：只可能是本机的回执被确认了。
                record.settled = true;
                record.not_before = None;
            }
            // 其余的情况本机不用改：文件还在等着收，或者那是只有发出去的传输才会有的结局。
        });
    }

    /// 服务端已经不认这条记录了（列表里找不到）：本机也不必再留着它。
    pub fn forget_transfer(&self, id: &str) {
        self.update(|view| view.transfers.retain(|record| record.id() != id));
    }

    /// 接收任务该做的下一件事。
    pub fn next_job(&self) -> Next {
        let view = self.view.lock().unwrap_or_else(PoisonError::into_inner);
        pick(&view.transfers, Instant::now())
    }

    /// 等待传输记录发生变化。
    pub async fn wait_for_transfers(&self) {
        self.transfers_changed.notified().await;
    }

    /// 领取一个排着队的文件：从这一刻起它算"正在接收"。
    /// 记录已经不在了、或者已经不该接收时返回 `None`。
    pub fn begin_receive(&self, id: &str) -> Option<SendRecord> {
        let mut taken = None;
        self.update(|view| {
            if let Some(record) = view.record(id)
                && record.view.phase == TransferPhase::Queued
            {
                record.view.phase = TransferPhase::InProgress;
                record.view.progress = 0.0;
                record.not_before = None;
                taken = Some(record.send.clone());
            }
        });
        taken
    }

    /// 这次接收可以被打断：对面撤回时按标识找到它。
    pub fn track_receive(&self, id: &str, cancel: CancellationToken) -> TrackedReceive<'_, R> {
        self.active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((id.to_owned(), cancel.clone()));
        TrackedReceive {
            state: self,
            id: id.to_owned(),
        }
    }

    /// 停下正在进行的某一次接收。没在进行时什么都不做。
    pub fn stop_receive(&self, id: &str) {
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

    /// 接收进度。只在整个百分比变了的时候才算一次变化，不然每来一段数据都要惊动界面。
    pub fn receive_progress(&self, id: &str, fraction: f64) {
        self.update(|view| {
            if let Some(record) = view.record(id)
                && record.view.phase == TransferPhase::InProgress
                // 整个百分点不同才算变化：两个整数相差至少 1，结果才不一样。
                && (percent(record.view.progress) - percent(fraction)).abs() >= 1.0
            {
                record.view.progress = fraction.clamp(0.0, 1.0);
            }
        });
    }

    /// 文件已经存好，回执还没发。
    pub fn receive_completed(&self, id: &str, received: &Received, handle: i32) {
        self.update(|view| {
            if let Some(record) = view.record(id)
                && record.view.phase == TransferPhase::InProgress
            {
                record.view.phase = TransferPhase::Complete;
                // 名字被改写或加了序号时，如实告诉用户文件实际叫什么。
                record.view.saved_as =
                    (received.name != record.view.name).then(|| received.name.clone());
                record.view.progress = 1.0;
                record.settled = false;
                record.attempts = 0;
                record.not_before = None;
                record.saved_path = Some(received.path.clone());
                record.handle = Some(handle);
            }
        });
        self.transfers_changed.notify_one();
    }

    /// 收不下来。回执还没发。
    pub fn receive_failed(&self, id: &str, failure: ReceiveFailure) {
        self.update(|view| {
            if let Some(record) = view.record(id)
                && record.view.phase == TransferPhase::InProgress
            {
                record.view.phase = TransferPhase::Failed;
                record.view.failure = Some(failure.into());
                record.view.progress = 0.0;
                record.settled = false;
                record.attempts = 0;
                record.not_before = None;
            }
        });
        self.transfers_changed.notify_one();
    }

    /// 这次没成，过一会儿再从头收一遍：多半是网络断了。
    pub fn retry_receive(&self, id: &str) {
        let now = Instant::now();
        self.update(|view| {
            if let Some(record) = view.record(id)
                && record.view.phase == TransferPhase::InProgress
            {
                record.view.phase = TransferPhase::Queued;
                record.view.progress = 0.0;
                record.attempts += 1;
                record.not_before = Some(now + backoff(record.attempts));
            }
        });
    }

    /// 回执没发出去，过一会儿再补。
    pub fn retry_settle(&self, id: &str) {
        let now = Instant::now();
        self.update(|view| {
            if let Some(record) = view.record(id)
                && !record.settled
                && record.view.phase.is_finished()
            {
                record.attempts += 1;
                // 补不回去的次数用完就不再试：服务端到时间会自己把它标成过期。
                record.settled = record.attempts >= SETTLE_ATTEMPTS;
                record.not_before = Some(now + backoff(record.attempts));
            }
        });
    }

    /// 回执发出去了，或者服务端本来就知道结局。
    pub fn transfer_settled(&self, id: &str) {
        self.update(|view| {
            if let Some(record) = view.record(id) {
                record.settled = true;
                record.not_before = None;
            }
        });
    }

    /// 需要补发回执的记录：它已知的结局，以及该报告什么。
    pub fn settlement_of(&self, id: &str) -> Option<(SendRecord, Option<ReceiveFailure>)> {
        self.view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .record(id)
            .filter(|record| !record.settled && record.view.phase.is_finished())
            .map(|record| {
                let report = match record.view.phase {
                    TransferPhase::Complete => None,
                    _ => record.view.failure.map(ReceiveFailure::from),
                };
                (record.send.clone(), report)
            })
    }

    /// 一个已经收到的文件在本机的位置。没收到、或者记录已经不在了时为 `None`。
    pub fn saved_file(&self, id: &str) -> Option<PathBuf> {
        self.view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .record(id)
            .filter(|record| record.view.phase == TransferPhase::Complete)
            .and_then(|record| record.saved_path.clone())
    }

    /// 用户点了某条通知：它说的那个文件在哪。
    pub fn saved_file_for_handle(&self, handle: i32) -> Option<PathBuf> {
        self.view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .transfers
            .iter()
            .find(|record| record.handle == Some(handle))
            .and_then(|record| record.saved_path.clone())
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

impl TransferPhase {
    /// 不会再自己变了：收好了、失败了，或者对面撤回了。
    pub fn is_finished(self) -> bool {
        !matches!(self, Self::Queued | Self::InProgress)
    }
}

impl TransferRecord {
    /// 服务端还在等这个文件，而本机已经走到某一步：接着做，不从头再来。
    fn catch_up(&mut self, send: &SendRecord) {
        match self.view.phase {
            // 还在排队或正在接收：它自己会走到头。
            TransferPhase::Queued | TransferPhase::InProgress => {}
            // 文件已经存好（或者已经收不下来），服务端却还在等：回执没送到，再发一次。
            TransferPhase::Complete | TransferPhase::Failed => {
                self.settled = false;
                self.attempts = 0;
                self.not_before = None;
            }
            // 本机以为对面撤回了，服务端却还在等：以服务端为准，重新收。
            TransferPhase::Cancelled | TransferPhase::Expired => *self = Self::queued(send),
        }
        self.send = send.clone();
    }
}

/// 一个正在接收的文件：离开作用域时就不再跟踪它。
pub struct TrackedReceive<'a, R: tauri::Runtime> {
    state: &'a AppState<R>,
    id: String,
}

impl<R: tauri::Runtime> Drop for TrackedReceive<'_, R> {
    fn drop(&mut self) {
        self.state
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(tracked, _)| tracked != &self.id);
    }
}

/// 进度取整到个百分点。
fn percent(fraction: f64) -> f64 {
    (fraction.clamp(0.0, 1.0) * 100.0).floor()
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
    use assemble_protocol::sends::{Direction, SendState};

    use super::*;
    use std::path::Path;

    /// 与 TypeScript 侧共用的状态样本：`apps/desktop/contract/snapshots.json`。
    const SAMPLES: &str = include_str!("../../contract/snapshots.json");

    /// 别的设备发来的、还在等着接收的一个文件。
    fn offered(id: &str) -> SendRecord {
        SendRecord {
            id: id.into(),
            direction: Direction::In,
            peer_device_id: "d2".into(),
            peer_device_name: "工作电脑".into(),
            name: "合同.pdf".into(),
            size: 1024,
            mime_type: None,
            sha256: "a".repeat(64),
            state: SendState::Offered,
            failure: None,
            created_at: Timestamp::now(),
            updated_at: Timestamp::now(),
        }
    }

    fn changed(record: &mut TransferRecord, change: impl FnOnce(&mut TransferRecord)) {
        change(record);
    }

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
    fn the_worker_takes_files_in_order_and_only_when_they_are_due() {
        let now = Instant::now();
        let first = TransferRecord::queued(&offered("a"));
        let mut second = TransferRecord::queued(&offered("b"));
        second.not_before = Some(now + Duration::from_secs(5));
        // 记录是新的在前；倒着看才是先来后到。
        assert_eq!(
            pick(&[second.clone(), first.clone()], now),
            Next::Work(Job::Receive { id: "a".into() })
        );
        assert_eq!(
            pick(&[second.clone()], now),
            Next::Wait(Duration::from_secs(5))
        );
        // 到点了就可以收。
        let later = now + Duration::from_secs(5);
        assert_eq!(
            pick(&[second.clone()], later),
            Next::Work(Job::Receive { id: "b".into() })
        );
        assert_eq!(pick(&[], now), Next::Nothing);

        // 正在接收的不算待办。
        let mut busy = first;
        changed(&mut busy, |record| {
            record.view.phase = TransferPhase::InProgress;
        });
        assert_eq!(pick(&[busy], now), Next::Nothing);
    }

    #[test]
    fn receipts_go_out_after_the_files_and_stop_once_the_server_knows() {
        let now = Instant::now();
        let queued = TransferRecord::queued(&offered("a"));
        let mut saved = TransferRecord::queued(&offered("b"));
        changed(&mut saved, |record| {
            record.view.phase = TransferPhase::Complete;
        });
        let mut failed = TransferRecord::queued(&offered("c"));
        changed(&mut failed, |record| {
            record.view.phase = TransferPhase::Failed;
            record.view.failure = Some(TransferFailure::DiskFull);
        });
        // 先收文件，再补回执。
        let all = vec![failed.clone(), saved.clone(), queued];
        assert_eq!(pick(&all, now), Next::Work(Job::Receive { id: "a".into() }));
        assert_eq!(
            pick(&[failed.clone(), saved.clone()], now),
            Next::Work(Job::Settle { id: "b".into() })
        );
        assert_eq!(
            pick(&[failed], now),
            Next::Work(Job::Settle { id: "c".into() })
        );

        // 服务端已经知道结局：不再补。
        let mut known = saved;
        changed(&mut known, |record| record.settled = true);
        assert_eq!(pick(&[known], now), Next::Nothing);

        // 撤回、过期的记录没有回执可补。
        for phase in [TransferPhase::Cancelled, TransferPhase::Expired] {
            let mut settled = TransferRecord::queued(&offered("d"));
            changed(&mut settled, |record| {
                record.view.phase = phase;
                record.settled = true;
            });
            assert_eq!(pick(&[settled], now), Next::Nothing, "{phase:?}");
        }
    }

    #[test]
    fn a_file_the_server_still_waits_for_is_continued_rather_than_started_over() {
        // 正在接收：接着收，进度不动。
        let mut record = TransferRecord::queued(&offered("a"));
        changed(&mut record, |record| {
            record.view.phase = TransferPhase::InProgress;
            record.view.progress = 0.4;
        });
        record.catch_up(&offered("a"));
        assert_eq!(record.view.phase, TransferPhase::InProgress);
        assert!((record.view.progress - 0.4).abs() < 1e-9);

        // 文件存好了、服务端却还在等：说明回执没送到。清掉退避，再发一次。
        let mut record = TransferRecord::queued(&offered("a"));
        changed(&mut record, |record| {
            record.view.phase = TransferPhase::Complete;
            record.settled = true;
            record.attempts = 3;
            record.not_before = Some(Instant::now() + Duration::from_secs(60));
        });
        record.catch_up(&offered("a"));
        assert_eq!(record.view.phase, TransferPhase::Complete);
        assert!(!record.settled);
        assert_eq!(record.attempts, 0);
        assert!(record.not_before.is_none());

        // 本机以为对面撤回了，服务端却还在等：以服务端为准，重新排队。
        let mut record = TransferRecord::queued(&offered("a"));
        changed(&mut record, |record| {
            record.view.phase = TransferPhase::Cancelled;
            record.settled = true;
        });
        record.catch_up(&offered("a"));
        assert_eq!(record.view.phase, TransferPhase::Queued);
        assert!(!record.settled);
    }

    #[test]
    fn waiting_between_retries_gets_longer_up_to_five_minutes() {
        assert_eq!(backoff(0), Duration::from_secs(2));
        assert_eq!(backoff(1), Duration::from_secs(2));
        assert_eq!(backoff(3), Duration::from_secs(15));
        assert_eq!(backoff(4), Duration::from_secs(60));
        assert_eq!(backoff(40), Duration::from_secs(5 * 60));
    }

    #[test]
    fn the_interface_only_learns_a_receive_folders_name() {
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
    fn where_a_file_was_saved_stays_out_of_the_snapshot() {
        let mut view = View::initial(&ServerOrigin::development());
        let mut record = TransferRecord::queued(&offered("a"));
        changed(&mut record, |record| {
            record.view.phase = TransferPhase::Complete;
            record.view.progress = 1.0;
            record.view.saved_as = Some("合同 (2).pdf".to_owned());
            record.saved_path = Some(PathBuf::from(
                "/Users/example/Downloads/Assemble/合同 (2).pdf",
            ));
            record.handle = Some(7);
        });
        view.transfers.push(record);

        let snapshot = serde_json::to_value(view.snapshot(LaunchState::Disabled)).unwrap();
        assert_eq!(snapshot["transfers"][0]["savedAs"], "合同 (2).pdf");
        assert_eq!(snapshot["transfers"][0]["progress"], 1.0);
        let text = snapshot.to_string();
        for private in ["/Users/", "Downloads", "handle", "settled"] {
            assert!(!text.contains(private), "{private} must stay in the shell");
        }
    }

    #[test]
    fn every_phase_ends_up_finished_or_still_running() {
        for running in [TransferPhase::Queued, TransferPhase::InProgress] {
            assert!(!running.is_finished());
        }
        for finished in [
            TransferPhase::Complete,
            TransferPhase::Failed,
            TransferPhase::Cancelled,
            TransferPhase::Expired,
        ] {
            assert!(finished.is_finished());
        }
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
