// 界面能看到的全部状态，与 Rust 侧的 `state::Snapshot` 一一对应。
//
// 两边的形状由同一份样本共同校验：TypeScript 在编译期检查样本符合这里的类型，
// Rust 在测试里把同一样本读进来再原样写出去。任何一边改了形状，测试都会失败。
// 凭据、路径与系统诊断永远不在这里出现。

/** 运行桌面端的平台，与服务端的拼法一致。 */
export type Platform = "windows" | "macos" | "linux";

export interface AccountView {
  id: string;
  username: string;
  displayName: string;
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

/** 收到的文件存在哪里。只知道是默认位置还是用户选的文件夹，以及那个文件夹叫什么。 */
export interface ReceiveView {
  /** 用户选的文件夹的名字；用默认位置（下载/Assemble）时为 null。 */
  customFolder: string | null;
}

/** 一次传输的方向：`in` 是别的设备发来的，`out` 是这台设备发出去的。 */
export type TransferDirection = "in" | "out";

/** 一次传输走到了哪一步。 */
export type TransferPhase =
  | "queued"
  | "inProgress"
  | "complete"
  | "failed"
  | "cancelled"
  | "expired";

/** 传输为什么失败。与服务端报告给对端的原因一一对应。 */
export type TransferFailure =
  | "diskFull"
  | "writeFailed"
  | "verificationFailed"
  | "cancelled";

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
}

export interface Notice {
  kind: "info" | "error";
  message: string;
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
  service: ServiceView;
  session: SessionView;
  /** 账户里的全部设备，包括这台自己。没登录时为空。 */
  devices: DeviceView[];
  /** 最近的传输，新的在前。没登录时为空。 */
  transfers: TransferView[];
  receive: ReceiveView;
  launchAtLogin: LaunchState;
  notice: Notice | null;
}
