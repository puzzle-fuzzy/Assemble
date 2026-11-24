// 界面测试共用的替身与夹具：状态的样本，和一个记录调用的假桥接。
// 主窗口与发送面板的测试都用它，免得两边各维护一份。

import type { Bridge, BridgeError, UnlistenFn } from "@/lib/bridge";
import type {
  DeviceView,
  DriveView,
  LaunchState,
  ServiceView,
  Snapshot,
  TransferView,
} from "@/lib/types";

export const ACCOUNT = {
  id: "account-1",
  username: "example",
  displayName: "Example",
};

/** 这台电脑自己。 */
export const DEVICE: DeviceView = {
  id: "device-1",
  name: "MacBook-Pro",
  platform: "macos",
  online: true,
};

/** 账户里的另一台电脑。 */
export const OTHER: DeviceView = {
  id: "device-2",
  name: "工作电脑",
  platform: "windows",
  online: false,
};

export function service(overrides: Partial<ServiceView> = {}): ServiceView {
  return { target: "production", reachable: true, stream: "off", ...overrides };
}

type Overrides = Partial<Omit<Snapshot, "session">> & {
  phase?: Snapshot["session"]["phase"];
};

export function signedIn(
  account: typeof ACCOUNT = ACCOUNT,
): Snapshot["session"] {
  return { phase: "signedIn", account, device: { ...DEVICE } };
}

/** 云盘名单的一份样本：两个文件，空间用了一点。 */
export const DRIVE: DriveView = {
  items: [
    {
      id: "file-1",
      name: "季度报表.xlsx",
      size: 524288,
      createdAt: "2026-11-24T02:30:00.000Z",
    },
    {
      id: "file-2",
      name: "合同.pdf",
      size: 1048576,
      createdAt: "2026-11-23T09:05:00.000Z",
    },
  ],
  usedBytes: 1572864,
  limitBytes: 2147483648,
};

export function snapshot(overrides: Overrides = {}): Snapshot {
  const { phase, ...rest } = overrides;
  const session: Snapshot["session"] =
    phase === "signingIn"
      ? { phase: "signingIn" }
      : phase === "signedIn"
        ? signedIn()
        : { phase: "signedOut" };
  return {
    service: service(),
    session,
    // 没登录时 Rust 不会给出设备列表，也不会留着传输记录、草稿或云盘名单。
    // 登录之后连上服务时，Rust 会把云盘名单拿回来。
    devices: phase === "signedIn" ? [{ ...DEVICE }] : [],
    transfers: [],
    draft: null,
    drive: phase === "signedIn" ? DRIVE : null,
    receive: { customFolder: null },
    launchAtLogin: "disabled",
    notice: null,
    ...rest,
  };
}

/** 一条传输记录；不说明阶段时，是一个从别的设备收到、已经存好的文件。 */
export function transfer(overrides: Partial<TransferView> = {}): TransferView {
  return {
    id: "t1",
    direction: "in",
    peer: "工作电脑",
    name: "合同.pdf",
    size: 1536 * 1024,
    phase: "complete",
    progress: 1,
    savedAs: null,
    failure: null,
    retryable: false,
    target: "device",
    ...overrides,
  };
}

/** 记录调用、可注入失败的替身桥接。 */
export class FakeBridge implements Bridge {
  current: Snapshot;
  calls: string[] = [];
  failWith: BridgeError | null = null;
  pending = false;
  /** `sendToDevice` 交回的传输标识：面板拿它盯进度。 */
  sentAs: string[] = [];
  private listeners = new Set<(snapshot: Snapshot) => void>();

  constructor(initial: Snapshot) {
    this.current = initial;
  }

  private gate(): Promise<void> {
    this.pending = true;
    return new Promise((resolve) => {
      // 让调用方有机会观察"进行中"的状态。
      setTimeout(() => {
        this.pending = false;
        resolve();
      }, 0);
    });
  }

  private async answer<T>(name: string, result: T): Promise<T> {
    this.calls.push(name);
    if (this.failWith) throw this.failWith;
    await this.gate();
    return result;
  }

  snapshot() {
    return Promise.resolve(this.current);
  }

  signIn() {
    return this.answer("signIn", this.current);
  }

  cancelSignIn() {
    return this.answer("cancelSignIn", undefined);
  }

  signOut() {
    return this.answer("signOut", this.current);
  }

  renameDevice(deviceId: string, name: string) {
    return this.answer(`renameDevice:${deviceId}:${name}`, this.current);
  }

  removeDevice(deviceId: string) {
    return this.answer(`removeDevice:${deviceId}`, this.current);
  }

  dismissNotice() {
    return this.answer("dismissNotice", this.current);
  }

  chooseReceiveFolder() {
    return this.answer("chooseReceiveFolder", this.current);
  }

  resetReceiveFolder() {
    return this.answer("resetReceiveFolder", this.current);
  }

  revealReceived(transferId: string) {
    return this.answer(`revealReceived:${transferId}`, undefined);
  }

  startSend() {
    return this.answer("startSend", this.current);
  }

  sendToDevice(deviceId: string) {
    this.calls.push(`sendToDevice:${deviceId}`);
    if (this.failWith) throw this.failWith;
    return Promise.resolve(this.sentAs);
  }

  sendToDrive() {
    this.calls.push("sendToDrive");
    if (this.failWith) throw this.failWith;
    return Promise.resolve(this.sentAs);
  }

  driveRefresh() {
    return this.answer("driveRefresh", this.current);
  }

  driveUpload() {
    return this.answer("driveUpload", this.current);
  }

  driveDownload(fileId: string) {
    return this.answer(`driveDownload:${fileId}`, this.current);
  }

  driveDelete(fileId: string) {
    return this.answer(`driveDelete:${fileId}`, this.current);
  }

  closeSendPanel() {
    return this.answer("closeSendPanel", undefined);
  }

  openMain() {
    return this.answer("openMain", undefined);
  }

  cancelTransfer(transferId: string) {
    return this.answer(`cancelTransfer:${transferId}`, undefined);
  }

  retryTransfer(transferId: string) {
    return this.answer(`retryTransfer:${transferId}`, undefined);
  }

  launchAtLogin() {
    return this.answer<LaunchState>(
      "launchAtLogin",
      this.current.launchAtLogin,
    );
  }

  setLaunchAtLogin(enabled: boolean) {
    this.calls.push(`setLaunchAtLogin:${enabled}`);
    if (this.failWith) throw this.failWith;
    return Promise.resolve(this.current.launchAtLogin);
  }

  async subscribe(listener: (snapshot: Snapshot) => void): Promise<UnlistenFn> {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  get listenerCount() {
    return this.listeners.size;
  }

  /** 模拟 Rust 推来新的状态。 */
  push(next: Snapshot) {
    this.current = next;
    for (const listener of this.listeners) listener(next);
  }
}
