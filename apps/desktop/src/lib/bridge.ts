// 与 Rust 壳通信的唯一通道。
//
// 界面没有通用的网络或文件能力：只能调用这里列出的几个具名命令，
// 并接收 Rust 主动推来的状态。命令名与参数名必须和 `src-tauri/src/commands.rs` 一致。

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { isSnapshot } from "@/lib/guard";
import type { LaunchState, Snapshot } from "@/lib/types";

export type { UnlistenFn } from "@tauri-apps/api/event";

/** 状态变化时 Rust 发来的事件名。 */
export const SNAPSHOT_EVENT = "assemble://snapshot";

/** 命令没有完成。`code` 是稳定错误码，`message` 仅供诊断，不显示给用户。 */
export class BridgeError extends Error {
  readonly code: string;

  constructor(code: string, message: string) {
    super(message);
    this.name = "BridgeError";
    this.code = code;
  }
}

export interface Bridge {
  snapshot(): Promise<Snapshot>;
  /** 在系统浏览器里登录；命令在登录流程结束后返回。 */
  signIn(): Promise<Snapshot>;
  cancelSignIn(): Promise<void>;
  /** 退出登录；服务端确认撤销之后才返回。 */
  signOut(): Promise<Snapshot>;
  launchAtLogin(): Promise<LaunchState>;
  setLaunchAtLogin(enabled: boolean): Promise<LaunchState>;
  /** 给账户里的另一台设备改名。 */
  renameDevice(deviceId: string, name: string): Promise<Snapshot>;
  /** 移除账户里的另一台设备，它的登录随之撤销。移除自己请用 signOut。 */
  removeDevice(deviceId: string): Promise<Snapshot>;
  /** 让用户挑一个接收文件夹。位置由系统选择器直接交给 Rust，不经过界面。 */
  chooseReceiveFolder(): Promise<Snapshot>;
  /** 回到默认的接收位置（下载/Assemble）。 */
  resetReceiveFolder(): Promise<Snapshot>;
  /** 在访达或资源管理器里显示一个已经收到的文件。界面只给得出传输的标识。 */
  revealReceived(transferId: string): Promise<void>;
  /** 提示看过了，收起来。 */
  dismissNotice(): Promise<Snapshot>;
  /** 订阅状态变化，返回取消订阅的函数。 */
  subscribe(listener: (snapshot: Snapshot) => void): Promise<UnlistenFn>;
}

export const bridge: Bridge = {
  snapshot: () => checked(() => invoke<unknown>("snapshot")),
  signIn: () => checked(() => invoke<unknown>("sign_in")),
  cancelSignIn: () => invoke<void>("cancel_sign_in"),
  signOut: () => checked(() => invoke<unknown>("sign_out")),
  launchAtLogin: () => invoke<LaunchState>("launch_at_login"),
  setLaunchAtLogin: (enabled) =>
    invoke<LaunchState>("set_launch_at_login", { enabled }),
  renameDevice: (deviceId, name) =>
    checked(() => invoke<unknown>("rename_device", { deviceId, name })),
  removeDevice: (deviceId) =>
    checked(() => invoke<unknown>("remove_device", { deviceId })),
  chooseReceiveFolder: () =>
    checked(() => invoke<unknown>("choose_receive_folder")),
  resetReceiveFolder: () =>
    checked(() => invoke<unknown>("reset_receive_folder")),
  revealReceived: (transferId) =>
    invoke<void>("reveal_received", { transferId }),
  dismissNotice: () => checked(() => invoke<unknown>("dismiss_notice")),
  subscribe: (listener) =>
    listen<unknown>(SNAPSHOT_EVENT, (event) => {
      listener(requireSnapshot(event.payload));
    }),
};

/** 取回一份状态，并在交界处核对它的形状。 */
async function checked(load: () => Promise<unknown>): Promise<Snapshot> {
  return requireSnapshot(await run(load));
}

function requireSnapshot(value: unknown): Snapshot {
  if (isSnapshot(value)) return value;
  throw new BridgeError(
    "INVALID_STATE",
    "the state sent by the desktop shell has an unknown shape",
  );
}

/** 把 Rust 侧拒绝命令时的任意值收敛成 `BridgeError`。 */
export function toBridgeError(error: unknown): BridgeError {
  if (error instanceof BridgeError) return error;
  if (typeof error === "object" && error !== null && "code" in error) {
    const { code, message } = error as { code: unknown; message: unknown };
    if (typeof code === "string") {
      return new BridgeError(code, typeof message === "string" ? message : "");
    }
  }
  return new BridgeError("UNKNOWN", typeof error === "string" ? error : "");
}

/** 执行一条命令，并把任何失败收敛成 `BridgeError`。 */
export async function run<T>(operation: () => Promise<T>): Promise<T> {
  try {
    return await operation();
  } catch (error) {
    throw toBridgeError(error);
  }
}
