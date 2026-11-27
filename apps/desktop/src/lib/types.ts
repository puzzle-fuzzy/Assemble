// 界面能看到的全部状态，与 Rust 侧的 `state::Snapshot` 一一对应。
//
// 两边的形状由同一份样本共同校验：TypeScript 在编译期检查样本符合这里的类型，
// Rust 在测试里把同一样本读进来再原样写出去。任何一边改了形状，测试都会失败。
// 凭据与系统诊断不在这里出现；Windows 接收目录仅作为只读展示文字给出。

/** 设备运行的平台，与服务端的拼法一致。手机应用是 `ios`（按操作系统命名）；服务端这一版不认识的值归成 `other`。 */
export type Platform = "windows" | "macos" | "linux" | "ios" | "other";

export interface AccountView {
  id: string;
  username: string;
  displayName: string;
  email: string | null;
  avatarUrl: string | null;
}

/** 一台设备在界面上的样子。 */
export interface DeviceView {
  id: string;
  name: string;
  platform: Platform;
  online: boolean;
}

/** 与服务端之间那条事件流的状态。别的设备在不在线，靠它才知道。 */
export type StreamState = "off" | "connecting" | "connected" | "reconnecting";

export type SessionView =
  | { phase: "signedOut" }
  | { phase: "signingIn" }
  | { phase: "signedIn"; account: AccountView; device: DeviceView };

export type LaunchState = "enabled" | "disabled" | "unsupported" | "unknown";

/** 接收位置的显示信息：Windows 给出只读目录文字，Mac 保留简称。 */
export interface ReceiveView {
  /** 用户选的文件夹的名字；用默认位置（下载/Assemble）时为 null。 */
  customFolder: string | null;
  /** Windows 接收目录的显示文字；不作为任何文件操作的命令参数。 */
  displayPath: string | null;
}

/** 一次传输的方向：`in` 是别的设备发来的，`out` 是这台设备发出去的。 */
export type TransferDirection = "in" | "out";

/** 一次传输走到了哪一步。收与发共用一套说法，文案按方向各自措辞。 */
export type TransferPhase =
  /** 排着队还没开始，或者上一次没成功、等着再试。 */
  | "queued"
  /** 正在收，或者正在发。 */
  | "inProgress"
  /** 发出去了，文件在服务端，等对方接收。只出现在发出去的传输上。 */
  | "waiting"
  /** 收到的文件已经存好；发出去的文件对方已经收下。 */
  | "complete"
  | "failed"
  /** 对方拒收了。只出现在发出去的传输上。 */
  | "declined"
  /** 发送方撤回了。 */
  | "cancelled"
  /** 过了时限还没送达。 */
  | "expired";

/** 传输为什么失败。前四种与服务端报告给对端的原因一一对应，其余几种只出现在发出去的传输上。 */
export type TransferFailure =
  | "diskFull"
  | "writeFailed"
  | "verificationFailed"
  | "cancelled"
  /** 点了发送之后对方下线了，文件没有发出去。 */
  | "deviceOffline"
  /** 超过单文件上限。 */
  | "tooLarge"
  /** 要发的文件读不出来了：被移走、被改动，或者没有权限。 */
  | "fileUnreadable"
  /** 服务端拒绝了这次发送。 */
  | "rejected"
  /** 云盘空间不够。 */
  | "quotaExceeded"
  /** 云盘里的文件在取回之前就被删掉了。 */
  | "vanished"
  /** 应用退出前尚未完成，重启后保留记录但不恢复旧文件任务。 */
  | "interrupted";

/** 一条传输的另一头是谁。 */
export type TransferTarget = /** 一台设备。 */ "device" /** 云盘。 */ | "drive";

/** 一次传输在界面上的样子。 */
export interface TransferView {
  /** 服务端那一次发送的标识。界面只能用它引用这条传输。 */
  id: string;
  direction: TransferDirection;
  /** 对端设备的名字，是传输当时的。 */
  peer: string;
  name: string;
  size: number;
  phase: TransferPhase;
  /** 0 到 1。只在 `inProgress` 期间变化。 */
  progress: number;
  /** 实际保存下来的文件名；与原名字不同时才有。 */
  savedAs: string | null;
  failure: TransferFailure | null;
  /** 能不能再发一次。规则在 Rust 里，界面照着显示按钮。 */
  retryable: boolean;
  /** 另一头是谁。文案据此措辞。 */
  target: TransferTarget;
}

export interface DriveFileView {
  id: string;
  name: string;
  size: number;
  /** 放进云盘的时间。 */
  createdAt: string;
}

/** 云盘里的文件与空间。 */
export interface DriveView {
  /** 新的在前。 */
  items: DriveFileView[];
  usedBytes: number;
  limitBytes: number;
}

export interface DraftFile {
  name: string;
  size: number;
}

/** 用户挑好、还没决定发给谁的文件。文件在哪不经过界面。 */
export interface DraftView {
  /** 这份草稿的标识。换了一份草稿，它就变了。 */
  id: string;
  files: DraftFile[];
}

export interface Notice {
  kind: "info" | "error";
  message: string;
}

/** 一条日程的重复规则。每月按北京时间日历算，短月落在月末。 */
export type Repeat = "none" | "daily" | "weekly" | "monthly";

/** 一处地点：给人看的地名，和拿去导航的地址。 */
export interface Location {
  label: string;
  address: string;
}

/**
 * 新建一条日程、或者整份替换它的内容时给出的东西。时间都是 UTC 的 RFC 3339。
 * 可选的字段省略就是"没有"。
 */
export interface ScheduleContent {
  title: string;
  location?: Location | null;
  startAt: string;
  endAt?: string | null;
  repeat?: Repeat;
  repeatUntil?: string | null;
  /** 提前多少分钟提醒；不给就是到点才提醒。 */
  remindBefore?: number | null;
  notes?: string | null;
}

/** 新建一条提醒、或者整份替换它的内容时给出的东西。 */
export interface ReminderContent {
  title: string;
  dueAt: string;
  notes?: string | null;
}

/** 一条提醒的状态。`fired` 是到点了、还没说"看过了"。 */
export type ReminderStatus = "scheduled" | "fired" | "dismissed" | "cancelled";

/** 还在进行中的一条日程。 */
export interface ScheduleView {
  id: string;
  /** 每改一次加一。改动时要带上看到的那个版本。 */
  version: number;
  /** 建立它的那台设备当时叫什么：助手写的和手建的，分得开。 */
  createdBy: string;
  title: string;
  location: Location | null;
  startAt: string;
  endAt: string | null;
  repeat: Repeat;
  repeatUntil: string | null;
  /** 提前多少分钟提醒；到点才提醒时为 null。 */
  remindBefore: number | null;
  notes: string | null;
  /** 一次性未来安排保留开始时刻；重复日程展开七天，按时间排。 */
  occurrences: string[];
}

/** 一条还没办完的提醒。 */
export interface ReminderView {
  id: string;
  version: number;
  status: ReminderStatus;
  createdBy: string;
  title: string;
  dueAt: string;
  notes: string | null;
}

/** 一条还没读的通知。 */
export interface NotificationView {
  id: string;
  title: string;
  body: string | null;
  /** 它是哪一条日程或提醒；对不上列表里的任何一条时为 null。 */
  scheduleId: string | null;
  reminderId: string | null;
  raisedAt: string;
}

/** 日程面板：还没读的通知、还在进行中的日程与提醒。 */
export interface PlannerView {
  navigation?: {
    resource: "schedule" | "reminder";
    id: string;
    locations: Location[];
  }[];
  /** 新的在前。 */
  notifications: NotificationView[];
  /** 最近的一场在前的排前面。 */
  schedules: ScheduleView[];
  /** 按到点时间排。 */
  reminders: ReminderView[];
  /** 点开某条通知之后要定位到那一条；没有要定位的时为 null。 */
  focus: string | null;
  /** 现在要摆出哪张新建表单（从托盘的"新建提醒…"进来时）；没有时为 null。 */
  compose: "reminder" | "schedule" | null;
}

/** 连接的是哪一套服务。由构建类型决定，不从界面输入。 */
export type ServiceTarget = "production" | "development";

export interface ServiceView {
  target: ServiceTarget;
  /** 还没探测过时为 null。 */
  reachable: boolean | null;
  stream: StreamState;
}

export interface Snapshot {
  remote: import("./remote").RemoteView;
  systemNotifications: import("@assemble/ui/lib/system-notifications").SystemNotifications;
  downloads: import("./downloads").DownloadsView;
  service: ServiceView;
  session: SessionView;
  /** 账户里的全部设备，包括这台自己。没登录时为空。 */
  devices: DeviceView[];
  /** 最近的传输，新的在前。没登录时为空。 */
  transfers: TransferView[];
  /** 用户挑好、还没决定发给谁的文件。没有草稿时为 null。 */
  draft: DraftView | null;
  /** 云盘里的文件与空间。没登录时为 null。 */
  drive: DriveView | null;
  /** Windows 右键菜单"用 Assemble 发送"的状态。以系统里的实际状态为准，不以点击为准。 */
  sendMenu: SendMenuState;
  receive: ReceiveView;
  launchAtLogin: LaunchState;
  notice: Notice | null;
  /** 日程、提醒与还没读的通知。没登录、或名单还没拿回来时为 null。 */
  planner: PlannerView | null;
  /** 这台电脑到点时不弹提醒。 */
  plannerMuted: boolean;
  /** 自动更新走到了哪一步。更新只从 Rust 那边驱动，界面不碰更新插件。 */
  update: UpdateView;
}

/** 自动更新走到了哪一步。 */
export type UpdateState =
  /** 这个构建不自动更新：连的不是正式服务。 */
  | "unsupported"
  /** 还没查过。 */
  | "idle"
  | "checking"
  /** 已经是最新，或者服务端还没有发布给这台电脑的更新。 */
  | "upToDate"
  | "downloading"
  /** 新版已经在手（macOS 上应用包已经换掉），下一次启动就是它。 */
  | "ready"
  /** 上一步没成；原因在 error 里。 */
  | "failed";

export interface UpdateView {
  state: UpdateState;
  /** 新版本的版本号；还没查到新版时为 null。 */
  version: string | null;
  /** 已经下载了多少字节、总共多少；没在下载时为 null。 */
  received: number | null;
  total: number | null;
  /** 上一次查完的时刻（纪元毫秒）；还没查过时为 null。 */
  checkedAt: number | null;
  /** 失败时的稳定错误码。 */
  error: string | null;
}

/** Windows 右键菜单"用 Assemble 发送"在这台电脑上的状态。 */
export type SendMenuState =
  | "enabled"
  | "disabled"
  /** 不是 Windows，或者未打包的开发运行。 */
  | "unsupported";
