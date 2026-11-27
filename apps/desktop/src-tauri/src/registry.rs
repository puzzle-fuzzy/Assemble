//! 传输记录表：这台设备收发过的每一个文件，以及它们各自走到了哪一步。
//!
//! 它同时是后台任务的队列：排队、重试、补发回执都从这张表里领活。
//! 这里只管"记住什么、下一步做什么"，不碰网络、文件与窗口，所以能单独测试；
//! 真正做事的在 `transfers.rs`。
//!
//! 一次传输只有一份记录，按方向各走各的路：
//!
//! ```text
//! 收到：queued ─▶ inProgress ─▶ complete ─▶（回执确认）
//!                            └▶ failed ─▶（把原因报告上去）
//! 发出：queued ─▶ inProgress ─▶ waiting ─▶ complete | declined | expired
//!                    └▶ failed              └▶ cancelled | failed
//! ```
//!
//! 文件在本机的位置只记在这里；交给界面的 [`TransferView`] 里没有它。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use assemble_client::OutgoingFile;
use assemble_device::inbox::ReceiveFailure;
use assemble_protocol::drive::DriveFile;
use assemble_protocol::primitives::Timestamp;
use assemble_protocol::sends::{Direction, FailReason, Send as SendRecord, SendState};
use serde::{Deserialize, Serialize};

use crate::state::DeviceView;

mod history;
pub use history::History;

/// 一次传输走到了哪一步。收与发共用一套说法，界面按方向各自措辞。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferPhase {
    /// 排着队还没开始，或者上一次没成功、等着再试。
    Queued,
    /// 正在收，或者正在发。
    InProgress,
    /// 发出去了，文件在服务端，等对方接收。
    Waiting,
    /// 收到的文件已经存好；发出去的文件对方已经收下。
    Complete,
    Failed,
    /// 对方拒收了。
    Declined,
    /// 发送方撤回了。
    Cancelled,
    /// 过了时限还没送达。
    Expired,
}

impl TransferPhase {
    /// 还会自己变：没开始、正在传，或者对方还没决定收不收。
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::InProgress | Self::Waiting)
    }
}

/// 传输为什么失败。前四种与协议里报告给对端的原因一一对应，
/// 由下面的转换负责；状态形状一律用驼峰拼写，所以拼法与服务端那边的不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferFailure {
    DiskFull,
    WriteFailed,
    VerificationFailed,
    Cancelled,
    /// 点了发送之后对方下线了，文件没有发出去。
    DeviceOffline,
    /// 超过单文件上限。
    TooLarge,
    /// 要发的文件读不出来了：被移走、被改动，或者没有权限。
    FileUnreadable,
    /// 服务端拒绝了这次发送，说不上是上面哪一种原因。
    Rejected,
    /// 云盘空间不够。
    QuotaExceeded,
    /// 云盘里的文件在取回之前就被删掉了。
    Vanished,
    /// 应用退出时尚未完成；不会拿旧的文件路径自动重新发送。
    Interrupted,
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

impl From<FailReason> for TransferFailure {
    fn from(reason: FailReason) -> Self {
        match reason {
            FailReason::DiskFull => Self::DiskFull,
            FailReason::WriteFailed => Self::WriteFailed,
            FailReason::VerificationFailed => Self::VerificationFailed,
            FailReason::Cancelled => Self::Cancelled,
        }
    }
}

impl TransferFailure {
    /// 作为接收方报告给发送方的原因。只有收到的文件才会有这一步；
    /// 发出去的失败原因没有对应的说法，返回 `None`。
    pub fn reported(self) -> Option<ReceiveFailure> {
        match self {
            Self::DiskFull => Some(ReceiveFailure::DiskFull),
            Self::WriteFailed => Some(ReceiveFailure::WriteFailed),
            Self::VerificationFailed => Some(ReceiveFailure::VerificationFailed),
            Self::Cancelled => Some(ReceiveFailure::Cancelled),
            Self::DeviceOffline
            | Self::TooLarge
            | Self::FileUnreadable
            | Self::Rejected
            | Self::QuotaExceeded
            | Self::Vanished
            | Self::Interrupted => None,
        }
    }
}

/// 一次传输在界面上的样子。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferView {
    /// 这条传输的标识。界面拿它引用这条传输，别的什么都传不了。
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
    /// 能不能再发一次：只有"文件根本没到服务端"的失败才行。规则在 Rust 里，界面照着显示。
    pub retryable: bool,
    /// 另一头是谁：一台设备，还是云盘。文案据此措辞。
    pub target: TransferTarget,
}

/// 一条传输的另一头是谁。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferTarget {
    Device,
    Drive,
}

/// 用户挑好、还没决定发给谁的文件。位置只留在本机，界面看得到名字与大小。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftView {
    /// 这份草稿的标识。换了一份草稿，它就变了。
    pub id: String,
    pub files: Vec<DraftFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftFile {
    pub name: String,
    pub size: u64,
}

/// 一份待发送的草稿。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub id: String,
    pub files: Vec<OutgoingFile>,
}

/// 发到哪里去：一台设备，或者云盘。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    Device(DeviceView),
    Drive,
}

impl Destination {
    /// 这条传输的另一头是哪一类。
    fn target(&self) -> TransferTarget {
        match self {
            Self::Device(_) => TransferTarget::Device,
            Self::Drive => TransferTarget::Drive,
        }
    }

    /// 另一头在界面上的名字。
    fn peer(&self) -> String {
        match self {
            Self::Device(device) => device.name.clone(),
            Self::Drive => DRIVE_NAME.to_owned(),
        }
    }
}

/// 云盘在界面上的名字。它不是一个设备，所以由这里统一给出。
pub const DRIVE_NAME: &str = "云盘";

/// 这台设备要发出去的一个文件。
#[derive(Debug, Clone, PartialEq)]
pub struct Outgoing {
    pub destination: Destination,
    /// 发出去时用的名字、长度与摘要都从这里来。
    pub file: OutgoingFile,
    /// 服务端收下之后给的那条发送记录；发去云盘的、或者文件还没到服务端时为 `None`。
    pub send: Option<SendRecord>,
}

/// 一条记录是怎么来的。
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    /// 别的设备发来的。重新接收时要核对它的长度与摘要。
    Incoming(SendRecord),
    /// 从云盘取回来的。
    FromDrive(DriveFile),
    /// 这台设备发出去的。
    Outgoing(Outgoing),
    /// 重启后留下的历史，不持有任何可以重新上传的文件。
    History { send: Option<SendRecord> },
}

/// 要收下来的东西：别的设备发来的，或者云盘里的。
#[derive(Debug, Clone, PartialEq)]
pub enum Delivery {
    FromDevice(SendRecord),
    FromDrive(DriveFile),
}

/// 一条传输记录：界面看到的 [`TransferView`]，加上只有本机用得上的那部分。
#[derive(Debug, Clone, PartialEq)]
pub struct TransferRecord {
    pub view: TransferView,
    pub origin: Origin,
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
    /// 记录首次出现的时间，只保存在本机历史里。
    pub recorded_at: Timestamp,
}

impl TransferRecord {
    fn incoming(send: &SendRecord) -> Self {
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
                retryable: false,
                target: TransferTarget::Device,
            },
            origin: Origin::Incoming(send.clone()),
            attempts: 0,
            not_before: None,
            settled: false,
            saved_path: None,
            handle: None,
            recorded_at: send.created_at,
        }
    }

    fn outgoing_record(id: String, destination: &Destination, file: OutgoingFile) -> Self {
        Self {
            view: TransferView {
                id,
                direction: Direction::Out,
                peer: destination.peer(),
                name: file.name(),
                size: file.size,
                phase: TransferPhase::Queued,
                progress: 0.0,
                saved_as: None,
                failure: None,
                retryable: false,
                target: destination.target(),
            },
            origin: Origin::Outgoing(Outgoing {
                destination: destination.clone(),
                file,
                send: None,
            }),
            attempts: 0,
            not_before: None,
            settled: false,
            saved_path: None,
            handle: None,
            recorded_at: Timestamp::now(),
        }
    }

    /// 排队去云盘里取一个文件回来。
    fn from_drive(file: &DriveFile) -> Self {
        Self {
            view: TransferView {
                id: format!("drive-{}", file.id),
                direction: Direction::In,
                peer: DRIVE_NAME.to_owned(),
                name: file.name.clone(),
                size: file.size,
                phase: TransferPhase::Queued,
                progress: 0.0,
                saved_as: None,
                failure: None,
                retryable: false,
                target: TransferTarget::Drive,
            },
            origin: Origin::FromDrive(file.clone()),
            attempts: 0,
            not_before: None,
            // 取回来不欠服务端任何回执。
            settled: true,
            saved_path: None,
            handle: None,
            recorded_at: Timestamp::now(),
        }
    }

    pub fn id(&self) -> &str {
        &self.view.id
    }

    pub fn incoming_record(&self) -> Option<&SendRecord> {
        match &self.origin {
            Origin::Incoming(send) => Some(send),
            Origin::History { send } if self.view.direction == Direction::In => send.as_ref(),
            Origin::FromDrive(_) | Origin::Outgoing(_) | Origin::History { .. } => None,
        }
    }

    pub fn outgoing(&self) -> Option<&Outgoing> {
        match &self.origin {
            Origin::Incoming(_) | Origin::FromDrive(_) | Origin::History { .. } => None,
            Origin::Outgoing(outgoing) => Some(outgoing),
        }
    }

    /// 别的设备发来的，还是云盘里的；只对要收下来的记录有意义。
    fn delivery(&self) -> Option<Delivery> {
        match &self.origin {
            Origin::Incoming(send) => Some(Delivery::FromDevice(send.clone())),
            Origin::FromDrive(file) => Some(Delivery::FromDrive(file.clone())),
            Origin::Outgoing(_) | Origin::History { .. } => None,
        }
    }

    /// 是不是要从别处收下来的记录。
    fn is_delivery(&self) -> bool {
        matches!(self.origin, Origin::Incoming(_) | Origin::FromDrive(_))
    }

    fn is_incoming(&self) -> bool {
        self.incoming_record().is_some()
    }

    /// 再发一次有没有意义：失败在文件到服务端之前，服务端那边什么都没有。
    fn can_retry(&self) -> bool {
        self.view.phase == TransferPhase::Failed
            && self
                .outgoing()
                .is_some_and(|outgoing| outgoing.send.is_none())
    }

    /// 服务端那一次发送的标识。
    fn server_id(&self) -> Option<&str> {
        match &self.origin {
            Origin::Incoming(send) => Some(&send.id),
            Origin::FromDrive(_) => None,
            Origin::Outgoing(outgoing) => outgoing.send.as_ref().map(|send| send.id.as_str()),
            Origin::History { send } => send.as_ref().map(|send| send.id.as_str()),
        }
    }

    /// 收到的文件有了本机的结局，而服务端还不知道：回执要补。
    fn needs_settlement(&self) -> bool {
        self.is_incoming()
            && !self.settled
            && matches!(
                self.view.phase,
                TransferPhase::Complete | TransferPhase::Failed
            )
    }

    /// 按服务端说的更新这条记录。
    fn apply_update(&mut self, send: &SendRecord) {
        let phase = match send.state {
            SendState::Offered => {
                // 收到的文件还在等着收，本机不用改；发出去的文件已在服务端，等对方接收。
                if self.view.direction == Direction::Out
                    && self.view.phase != TransferPhase::Cancelled
                {
                    self.view.phase = TransferPhase::Waiting;
                    self.view.progress = 1.0;
                }
                return;
            }
            SendState::Delivered => TransferPhase::Complete,
            SendState::Declined => TransferPhase::Declined,
            SendState::Cancelled => TransferPhase::Cancelled,
            SendState::Expired => TransferPhase::Expired,
            SendState::Failed => TransferPhase::Failed,
        };
        // 已经存到本机的文件以本机为准：对面这时候撤回，也撤不回已经落盘的文件。
        let already_saved = self.is_incoming()
            && matches!(phase, TransferPhase::Cancelled | TransferPhase::Expired)
            && matches!(
                self.view.phase,
                TransferPhase::Complete | TransferPhase::Failed
            );
        if already_saved {
            return;
        }
        self.view.phase = phase;
        self.view.failure = send.failure.map(TransferFailure::from);
        // 收到并确认过的文件进度就是 1；其余的结局都不再是"传了一部分"。
        self.view.progress = if phase == TransferPhase::Complete {
            1.0
        } else {
            0.0
        };
        self.settled = true;
        self.not_before = None;
        self.attempts = 0;
    }

    /// 服务端还在等这个收到的文件，而本机已经走到某一步：接着做，不从头再来。
    fn catch_up(&mut self, send: &SendRecord) {
        // 中断并不表示接收失败，不能把它当成成功回执；服务端仍在等时重新排队。
        if self.view.failure == Some(TransferFailure::Interrupted) {
            let recorded_at = self.recorded_at;
            *self = Self::incoming(send);
            self.recorded_at = recorded_at;
            return;
        }
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
            TransferPhase::Waiting
            | TransferPhase::Cancelled
            | TransferPhase::Declined
            | TransferPhase::Expired => {
                *self = Self::incoming(send);
            }
        }
        self.origin = Origin::Incoming(send.clone());
    }
}

/// 服务端说一次发送变了之后，本机还要不要做什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Updated {
    /// 本机没有这条记录，或者不用改。
    Ignored,
    Applied,
    /// 本机已经放弃了这次发送，可文件其实到了服务端：得去服务端撤回。
    Withdraw,
}

/// 取消一条传输之后，本机之外还要做什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cancellation {
    /// 到此为止：还没轮到它，或者它正停在一个改不了的状态。
    Nothing,
    /// 正在传的那一次已经停下，本机的记录已经改好。
    Local,
    /// 文件已经在服务端：要去服务端撤回，标识是服务端那一次发送的。
    OnServer(String),
}

/// 后台任务该做的下一件事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// 把一个排着队的文件收下来。
    Receive { id: String },
    /// 把一个排着队的文件发出去。
    Send { id: String },
    /// 把本机已经有的结局告诉服务端：收下了，或者收不下来。
    Settle { id: String },
}

/// 后台任务现在有没有事可做。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    Nothing,
    /// 现在没有，但这么多时间之后会有。
    Wait(Duration),
    Work(Job),
}

/// 传输记录最多留这么多条；超了就丢掉最旧的已经结束的记录。
const MAX_TRANSFERS: usize = 100;

/// 补回执最多这么多次。再多次服务端都不收，就只能等它自己过期。
const SETTLE_ATTEMPTS: u32 = 8;

/// 每多试一次等多久：2 秒、5 秒、15 秒、1 分钟，之后一直是 5 分钟。
pub(crate) fn backoff(attempts: u32) -> Duration {
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

/// 从一堆记录里挑出该做的下一件事。记录是新的在前，这里倒着看：先来后到。
///
/// 先收文件，再发文件，最后补回执：文件早点到用户手里比回执要紧；
/// 而收又排在发前面，因为对面正等着。
pub(crate) fn pick(records: &[TransferRecord], now: Instant) -> Next {
    let wait_of = |record: &TransferRecord| {
        record
            .not_before
            .map_or(Duration::ZERO, |time| time.saturating_duration_since(now))
    };
    let mut soonest = Duration::MAX;
    for incoming in [true, false] {
        for record in records.iter().rev() {
            if record.view.phase != TransferPhase::Queued || record.is_delivery() != incoming {
                continue;
            }
            let wait = wait_of(record);
            if wait.is_zero() {
                let id = record.view.id.clone();
                return Next::Work(if incoming {
                    Job::Receive { id }
                } else {
                    Job::Send { id }
                });
            }
            soonest = soonest.min(wait);
        }
    }
    for record in records.iter().rev() {
        if record.needs_settlement() {
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

/// 进度取整到个百分点。
pub(crate) fn percent(fraction: f64) -> f64 {
    (fraction.clamp(0.0, 1.0) * 100.0).floor()
}

/// 传输记录表。
#[derive(Debug, Clone, PartialEq)]
pub struct Registry {
    /// 最近的传输，新的在前。
    transfers: Vec<TransferRecord>,
    draft: Option<Draft>,
    /// 下一条本机自己编号的记录。
    next_local: u64,
    /// 下一份草稿的编号。
    next_draft: u64,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            transfers: Vec::new(),
            draft: None,
            next_local: 1,
            next_draft: 1,
        }
    }
}

impl Registry {
    pub fn views(&self) -> Vec<TransferView> {
        self.transfers
            .iter()
            .map(|record| {
                let mut view = record.view.clone();
                view.retryable = record.can_retry();
                view
            })
            .collect()
    }

    pub fn draft_view(&self) -> Option<DraftView> {
        self.draft.as_ref().map(|draft| DraftView {
            id: draft.id.clone(),
            files: draft
                .files
                .iter()
                .map(|file| DraftFile {
                    name: file.name(),
                    size: file.size,
                })
                .collect(),
        })
    }

    /// 与账户有关的东西都清掉。文件留在接收目录里，名单不留给下一个登录的人。
    pub fn clear(&mut self) {
        self.transfers.clear();
        self.draft = None;
    }

    fn record(&mut self, id: &str) -> Option<&mut TransferRecord> {
        self.transfers.iter_mut().find(|record| record.id() == id)
    }

    /// 按服务端那一次发送的标识找记录。
    fn by_server_id(&mut self, send_id: &str) -> Option<&mut TransferRecord> {
        self.transfers
            .iter_mut()
            .find(|record| record.server_id() == Some(send_id))
    }

    fn push(&mut self, record: TransferRecord) {
        self.transfers.insert(0, record);
        // 太多了就丢掉最旧的、已经结束的记录。
        if self.transfers.len() > MAX_TRANSFERS
            && let Some(index) = self
                .transfers
                .iter()
                .rposition(|record| !record.view.phase.is_active())
        {
            self.transfers.remove(index);
        }
    }

    // ---- 收到的 ----

    /// 别的设备发来了一个文件，或者服务端还在等这台设备处理它。
    /// 按本机已经走到哪一步接上，不会把同一个文件收两遍。
    pub fn offer(&mut self, send: &SendRecord) {
        // 只处理"发给我的、还在等着"的记录；别的情况由 [`Registry::updated`] 负责。
        if send.state != SendState::Offered || send.direction != Direction::In {
            return;
        }
        if let Some(record) = self.by_server_id(&send.id) {
            record.catch_up(send);
        } else {
            self.push(TransferRecord::incoming(send));
        }
    }

    /// 服务端说一次发送变了：撤回了、过期了、对方收下了，或者本机的回执被确认了。
    pub fn updated(&mut self, send: &SendRecord) -> Updated {
        let Some(record) = self.by_server_id(&send.id) else {
            return Updated::Ignored;
        };
        match &mut record.origin {
            // 云盘里取回的与服务端的发送记录无关。
            Origin::FromDrive(_) => Updated::Ignored,
            Origin::Incoming(_) => {
                record.apply_update(send);
                Updated::Applied
            }
            Origin::Outgoing(outgoing) => {
                outgoing.send = Some(send.clone());
                record.apply_update(send);
                // 用户已经取消了这次发送，可文件其实到了服务端。
                if record.view.phase == TransferPhase::Cancelled && !send.state.is_terminal() {
                    Updated::Withdraw
                } else {
                    Updated::Applied
                }
            }
            Origin::History { send: stored } => {
                *stored = Some(send.clone());
                record.apply_update(send);
                Updated::Applied
            }
        }
    }

    /// 服务端还在等着交出去的这次发送，是不是本机已经放弃的那一个。
    ///
    /// 上传途中按了取消，文件其实可能已经到了服务端，只是回执没送到，本机没记下
    /// 服务端给的标识。内容摘要、文件名和收件人都一样的，就是它。
    pub fn abandoned(&self, send: &SendRecord) -> bool {
        self.transfers.iter().any(|record| {
            record.view.phase == TransferPhase::Cancelled
                && record.outgoing().is_some_and(|outgoing| {
                    outgoing.send.is_none()
                        && outgoing.file.sha256 == send.sha256
                        && outgoing.file.name() == send.name
                        && matches!(&outgoing.destination, Destination::Device(device) if device.id == send.peer_device_id)
                })
        })
    }

    /// 服务端已经不认这条记录了（列表里找不到）：本机也不必再留着它。
    pub fn forget(&mut self, id: &str) {
        self.transfers.retain(|record| record.id() != id);
    }

    pub fn next_job(&self, now: Instant) -> Next {
        pick(&self.transfers, now)
    }

    /// 排队去云盘里取一个文件。同一个文件已经排着、或者正在取时，不重复排。
    ///
    /// 之前取回过（或者没取成）的，新的一次顶替旧记录：同一个标识始终只有一条，
    /// 界面不会列出两个一样的。
    pub fn fetch_from_drive(&mut self, file: &DriveFile) -> String {
        let id = format!("drive-{}", file.id);
        if self
            .transfers
            .iter()
            .any(|record| record.id() == id && record.view.phase.is_active())
        {
            return id;
        }
        self.transfers.retain(|record| record.id() != id);
        self.push(TransferRecord::from_drive(file));
        id
    }

    /// 领取一个排着队的文件去收：别的设备发来的，或者云盘里的。
    /// 记录已经不在了、或者已经不该接收时返回 `None`。
    pub fn begin_receive(&mut self, id: &str) -> Option<Delivery> {
        let record = self.record(id)?;
        if record.view.phase != TransferPhase::Queued || !record.is_delivery() {
            return None;
        }
        record.view.phase = TransferPhase::InProgress;
        record.view.progress = 0.0;
        record.not_before = None;
        record.delivery()
    }

    /// 进度。只在整个百分比变了的时候才算一次变化，不然每来一段数据都要惊动界面。
    pub fn progress(&mut self, id: &str, fraction: f64) {
        if let Some(record) = self.record(id)
            && record.view.phase == TransferPhase::InProgress
            // 整个百分点不同才算变化：两个整数相差至少 1，结果才不一样。
            && (percent(record.view.progress) - percent(fraction)).abs() >= 1.0
        {
            record.view.progress = fraction.clamp(0.0, 1.0);
        }
    }

    /// 收到的文件已经存好，回执还没发。
    pub fn receive_completed(&mut self, id: &str, name: &str, path: PathBuf, handle: i32) {
        if let Some(record) = self.record(id)
            && record.view.phase == TransferPhase::InProgress
        {
            record.view.phase = TransferPhase::Complete;
            // 名字被改写或加了序号时，如实告诉用户文件实际叫什么。
            record.view.saved_as = (name != record.view.name).then(|| name.to_owned());
            record.view.progress = 1.0;
            record.settled = false;
            record.attempts = 0;
            record.not_before = None;
            record.saved_path = Some(path);
            record.handle = Some(handle);
        }
    }

    /// 收不下来。别的设备发来的，回执还没发。
    pub fn receive_failed(&mut self, id: &str, failure: TransferFailure) {
        if let Some(record) = self.record(id)
            && record.view.phase == TransferPhase::InProgress
            && record.is_delivery()
        {
            record.view.phase = TransferPhase::Failed;
            record.view.failure = Some(failure);
            record.view.progress = 0.0;
            // 从云盘取回的不欠回执。
            record.settled = !record.is_delivery() || matches!(record.origin, Origin::FromDrive(_));
            record.attempts = 0;
            record.not_before = None;
        }
    }

    /// 这次没成，过一会儿再从头收一遍：多半是网络断了。
    pub fn retry_receive(&mut self, id: &str, now: Instant) {
        if let Some(record) = self.record(id)
            && record.view.phase == TransferPhase::InProgress
        {
            record.view.phase = TransferPhase::Queued;
            record.view.progress = 0.0;
            record.attempts += 1;
            record.not_before = Some(now + backoff(record.attempts));
        }
    }

    /// 回执没发出去，过一会儿再补。
    pub fn retry_settle(&mut self, id: &str, now: Instant) {
        if let Some(record) = self.record(id)
            && !record.settled
            && matches!(
                record.view.phase,
                TransferPhase::Complete | TransferPhase::Failed
            )
        {
            record.attempts += 1;
            // 补不回去的次数用完就不再试：服务端到时间会自己把它标成过期。
            record.settled = record.attempts >= SETTLE_ATTEMPTS;
            record.not_before = Some(now + backoff(record.attempts));
        }
    }

    /// 回执发出去了，或者服务端本来就知道结局。
    pub fn settled(&mut self, id: &str) {
        if let Some(record) = self.record(id) {
            record.settled = true;
            record.not_before = None;
        }
    }

    /// 需要补发回执的记录：它对应的服务端记录，以及该报告什么。
    /// `None` 表示收下了，`Some` 是收不下来的原因。
    pub fn settlement_of(&self, id: &str) -> Option<(SendRecord, Option<ReceiveFailure>)> {
        let record = self.transfers.iter().find(|record| record.id() == id)?;
        if !record.needs_settlement() {
            return None;
        }
        let report = match record.view.phase {
            TransferPhase::Complete => None,
            _ => record.view.failure.and_then(TransferFailure::reported),
        };
        Some((record.incoming_record().cloned()?, report))
    }

    /// 一个已经收到的文件在本机的位置。没收到、或者记录已经不在了时为 `None`。
    pub fn saved_file(&self, id: &str) -> Option<PathBuf> {
        self.transfers
            .iter()
            .find(|record| record.id() == id)
            .filter(|record| record.view.phase == TransferPhase::Complete)
            .and_then(|record| record.saved_path.clone())
    }

    /// 用户点了某条通知：它说的那个文件在哪。
    pub fn saved_file_for_handle(&self, handle: i32) -> Option<PathBuf> {
        self.transfers
            .iter()
            .find(|record| record.handle == Some(handle))
            .and_then(|record| record.saved_path.clone())
    }

    // ---- 发出的 ----

    /// 用户挑好了要发的文件。原先挑好还没发的就作废。
    pub fn set_draft(&mut self, files: Vec<OutgoingFile>) -> DraftView {
        let id = format!("draft-{}", self.next_draft);
        self.next_draft += 1;
        let draft = Draft { id, files };
        let view = DraftView {
            id: draft.id.clone(),
            files: draft
                .files
                .iter()
                .map(|file| DraftFile {
                    name: file.name(),
                    size: file.size,
                })
                .collect(),
        };
        self.draft = Some(draft);
        view
    }

    /// 发送面板关掉了：挑好的文件不再留着。
    pub fn drop_draft(&mut self) {
        self.draft = None;
    }

    /// 把草稿里的文件排进队列，发往 `destination`。返回每一条记录的标识，先挑的在前。
    pub fn queue_draft(&mut self, destination: Destination) -> Vec<String> {
        let Some(draft) = self.draft.take() else {
            return Vec::new();
        };
        self.queue(destination, draft.files)
    }

    /// 把挑好的文件排进队列，发往 `destination`。返回每一条记录的标识，先挑的在前。
    pub fn queue(&mut self, destination: Destination, files: Vec<OutgoingFile>) -> Vec<String> {
        // 名单上离线的设备本来选不了；它恰好在点下去那一刻上线了也不影响，照样发。
        let destination = match destination {
            Destination::Device(mut device) => {
                device.online = true;
                Destination::Device(device)
            }
            Destination::Drive => Destination::Drive,
        };
        files
            .into_iter()
            .map(|file| {
                let id = loop {
                    let id = format!("out-{}", self.next_local);
                    self.next_local = self.next_local.wrapping_add(1).max(1);
                    if !self.transfers.iter().any(|record| record.id() == id) {
                        break id;
                    }
                };
                self.push(TransferRecord::outgoing_record(
                    id.clone(),
                    &destination,
                    file,
                ));
                id
            })
            .collect()
    }

    /// 文件存进云盘了。存进云盘的文件没有"等对方接收"这一步，存好就算完。
    pub fn drive_uploaded(&mut self, id: &str) -> bool {
        let Some(record) = self.record(id) else {
            return false;
        };
        if !matches!(record.origin, Origin::Outgoing(_))
            || record.view.phase != TransferPhase::InProgress
        {
            return false;
        }
        record.view.phase = TransferPhase::Complete;
        record.view.progress = 1.0;
        record.settled = true;
        record.not_before = None;
        record.attempts = 0;
        true
    }

    /// 领取一个排着队的文件去发。文件还在草稿里、记录已经不在了，或者不该再发时返回 `None`。
    pub fn begin_send(&mut self, id: &str) -> Option<Outgoing> {
        let record = self.record(id)?;
        if record.view.phase != TransferPhase::Queued {
            return None;
        }
        let outgoing = record.outgoing()?.clone();
        record.view.phase = TransferPhase::InProgress;
        record.view.progress = 0.0;
        record.not_before = None;
        Some(outgoing)
    }

    /// 服务端收下了文件，正在等对方接收。
    pub fn send_uploaded(&mut self, id: &str, send: &SendRecord) -> Updated {
        let Some(record) = self.record(id) else {
            return Updated::Ignored;
        };
        if let Origin::Outgoing(outgoing) = &mut record.origin {
            outgoing.send = Some(send.clone());
        } else {
            return Updated::Ignored;
        }
        // 用户在上传途中取消了：记录已经标成"已撤回"，不改回去；文件得去服务端撤回。
        if record.view.phase == TransferPhase::Cancelled {
            return Updated::Withdraw;
        }
        record.apply_update(send);
        Updated::Applied
    }

    /// 这一次没发出去，原因说得很清楚。
    pub fn send_failed(&mut self, id: &str, failure: TransferFailure) {
        if let Some(record) = self.record(id)
            && record.view.phase == TransferPhase::InProgress
            && record
                .outgoing()
                .is_some_and(|outgoing| outgoing.send.is_none())
        {
            record.view.phase = TransferPhase::Failed;
            record.view.failure = Some(failure);
            record.view.progress = 0.0;
            record.settled = true;
        }
    }

    /// 网络断了，过一会儿再从头发一遍。
    pub fn retry_send(&mut self, id: &str, now: Instant) {
        if let Some(record) = self.record(id)
            && record.view.phase == TransferPhase::InProgress
            && record
                .outgoing()
                .is_some_and(|outgoing| outgoing.send.is_none())
        {
            record.view.phase = TransferPhase::Queued;
            record.view.progress = 0.0;
            record.attempts += 1;
            record.not_before = Some(now + backoff(record.attempts));
        }
    }

    /// 再发一次：只对"文件根本没到服务端"的失败有用。对方拒收、过期了的，得重新发起。
    pub fn retry_failed_send(&mut self, id: &str, now: Instant) -> bool {
        let Some(record) = self.record(id) else {
            return false;
        };
        let retryable = record.view.phase == TransferPhase::Failed
            && record
                .outgoing()
                .is_some_and(|outgoing| outgoing.send.is_none());
        if retryable {
            record.view.phase = TransferPhase::Queued;
            record.view.failure = None;
            record.attempts = 0;
            record.not_before = Some(now);
        }
        retryable
    }

    /// 用户要取消一条传输。本机能停的部分在这里停掉，停不掉的告诉调用方。
    pub fn cancel(&mut self, id: &str) -> Cancellation {
        let Some(record) = self.record(id) else {
            return Cancellation::Nothing;
        };
        match (&record.origin, record.view.phase) {
            // 从云盘取回的：停下就行，不欠谁一个交代。
            (Origin::FromDrive(_), TransferPhase::Queued | TransferPhase::InProgress) => {
                record.view.phase = TransferPhase::Cancelled;
                record.view.progress = 0.0;
                record.settled = true;
                record.not_before = None;
                Cancellation::Local
            }
            // 还没开始收、或收到一半：停下，并把"用户取消了"报告给发送方。
            (Origin::Incoming(_), TransferPhase::Queued | TransferPhase::InProgress) => {
                record.view.phase = TransferPhase::Failed;
                record.view.failure = Some(TransferFailure::Cancelled);
                record.view.progress = 0.0;
                record.settled = false;
                record.attempts = 0;
                record.not_before = None;
                Cancellation::Local
            }
            // 还没发出去、或发了一半：到此为止，服务端那边什么都没有。
            (Origin::Outgoing(outgoing), TransferPhase::Queued | TransferPhase::InProgress)
                if outgoing.send.is_none() =>
            {
                record.view.phase = TransferPhase::Cancelled;
                record.view.progress = 0.0;
                record.settled = true;
                record.not_before = None;
                Cancellation::Local
            }
            // 文件已经在服务端等对方接收：去服务端撤回。
            (_, TransferPhase::Waiting) => match record.server_id() {
                Some(send_id) => Cancellation::OnServer(send_id.to_owned()),
                None => Cancellation::Nothing,
            },
            _ => Cancellation::Nothing,
        }
    }
}

#[cfg(test)]
mod tests;
