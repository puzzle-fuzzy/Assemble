// 界面能看到的全部状态，与 Rust 侧的 `state::Snapshot` 一一对应。
//
// 两边的形状由同一份样本共同校验：TypeScript 在编译期检查样本符合这里的类型，
// Rust 在测试里把同一样本读进来再原样写出去。任何一边改了形状，测试都会失败。
// 凭据、路径与系统诊断永远不在这里出现。

export interface AccountView {
  id: string;
  username: string;
  displayName: string;
}

export type SessionView =
  | { phase: "signedOut" }
  | { phase: "signingIn" }
  | { phase: "signedIn"; account: AccountView };

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
}

export interface Snapshot {
  service: ServiceView;
  session: SessionView;
  launchAtLogin: LaunchState;
  notice: Notice | null;
}
