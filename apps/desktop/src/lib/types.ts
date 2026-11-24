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
  launchAtLogin: LaunchState;
  notice: Notice | null;
}
