// 界面测试共用的替身与夹具：状态的样本，和一个记录调用的假桥接。
// 主窗口与发送面板的测试都用它，免得两边各维护一份。

import type { Bridge, BridgeError, UnlistenFn } from "@/lib/bridge";
import type {
  AccountView,
  DeviceView,
  DriveView,
  LaunchState,
  PlannerView,
  ReminderContent,
  ScheduleContent,
  ServiceView,
  Snapshot,
  TransferView,
  UpdateView,
} from "@/lib/types";

export const ACCOUNT: AccountView = {
  id: "account-1",
  username: "example",
  displayName: "Example",
  email: null,
  avatarUrl: null,
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

/**
 * 日程名单的一份样本：一条天天有的、一条一次性的（有地点、备注，提前半小时提醒）、
 * 一条每周的，一条已经过了点的提醒和一条没到的，两条没读的通知。
 */
export const PLANNER: PlannerView = {
  notifications: [
    {
      id: "reminder-1",
      title: "取快递",
      body: "驿站楼下",
      scheduleId: null,
      reminderId: "reminder-1",
      raisedAt: "2026-11-25T01:00:00.000Z",
    },
    {
      id: "schedule-1-1795544400000",
      title: "周会",
      body: "11月25日 09:00 开始\n三楼小会议室",
      scheduleId: "schedule-1",
      reminderId: null,
      raisedAt: "2026-11-25T01:00:00.000Z",
    },
  ],
  schedules: [
    {
      id: "schedule-1",
      version: 1,
      createdBy: "ZCode 的助手",
      title: "周会",
      location: null,
      startAt: "2026-11-25T01:00:00.000Z",
      endAt: "2026-11-25T02:00:00.000Z",
      repeat: "daily",
      repeatUntil: null,
      remindBefore: null,
      notes: "三楼小会议室",
      occurrences: [
        "2026-11-25T01:00:00.000Z",
        "2026-11-26T01:00:00.000Z",
        "2026-11-27T01:00:00.000Z",
      ],
    },
    {
      id: "schedule-2",
      version: 3,
      createdBy: "MacBook-Pro",
      title: "火车",
      location: { label: "上海虹桥站", address: "申虹路 1500 号" },
      startAt: "2026-11-26T01:00:00.000Z",
      endAt: null,
      repeat: "none",
      repeatUntil: null,
      remindBefore: 30,
      notes: "带身份证",
      occurrences: ["2026-11-26T01:00:00.000Z"],
    },
    {
      id: "schedule-3",
      version: 1,
      createdBy: "Codex 的助手",
      title: "周报",
      location: null,
      startAt: "2026-11-20T08:00:00.000Z",
      endAt: "2026-11-20T09:00:00.000Z",
      repeat: "weekly",
      repeatUntil: "2026-12-31T16:00:00.000Z",
      remindBefore: null,
      notes: null,
      occurrences: ["2026-11-27T08:00:00.000Z"],
    },
  ],
  reminders: [
    {
      id: "reminder-1",
      version: 2,
      status: "fired",
      createdBy: "ZCode 的助手",
      title: "取快递",
      dueAt: "2026-11-25T01:00:00.000Z",
      notes: "驿站楼下",
    },
    {
      id: "reminder-2",
      version: 1,
      status: "scheduled",
      createdBy: "MacBook-Pro",
      title: "给妈妈打电话",
      dueAt: "2026-11-26T12:00:00.000Z",
      notes: null,
    },
  ],
  focus: null,
  compose: null,
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
    systemNotifications: { items: [], available: true },
    remote: { sessions: [] },
    downloads: {
      assistantAllowed: true,
      browserTaskId: null,
      folderName: "网络下载",
      toolsAvailable: true,
      tasks: [],
    },
    session,
    // 没登录时 Rust 不会给出设备列表，也不会留着传输记录、草稿或云盘名单。
    // 登录之后连上服务时，Rust 会把云盘名单拿回来。
    devices: phase === "signedIn" ? [{ ...DEVICE }] : [],
    transfers: [],
    draft: null,
    drive: phase === "signedIn" ? DRIVE : null,
    // 测试跑在 macOS 的开发构建上：右键菜单在这里不存在。
    sendMenu: "unsupported",
    receive: { customFolder: null, displayPath: null },
    launchAtLogin: "disabled",
    notice: null,
    // 登录之后连上服务时，Rust 会把日程名单拿回来。
    planner: phase === "signedIn" ? PLANNER : null,
    plannerMuted: false,
    update: UPDATE,
    ...rest,
  };
}

/** 自动更新的状态：还没查过。 */
export const UPDATE: UpdateView = {
  state: "idle",
  version: null,
  received: null,
  total: null,
  checkedAt: null,
  error: null,
};

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
  downloadPreview: import("@/lib/downloads").DownloadPreview = {
    id: "preview",
    kind: "hls",
    title: "HLS",
    sourceHost: "example.com",
    durationSeconds: 10,
    variants: [{ id: "v0", width: null, height: null, bandwidth: null }],
  };
  async downloadPrepare(input: import("@/lib/downloads").DownloadInput) {
    this.calls.push("downloadPrepare");
    await this.gate();
    if (this.failWith) throw this.failWith;
    this.downloadInput = input;
    return this.downloadPreview;
  }
  downloadInput: import("@/lib/downloads").DownloadInput | null = null;
  async downloadImportHls(_base: string | null, _page: string | null) {
    this.calls.push("downloadImportHls");
    return this.downloadPreview;
  }
  async downloadStart(_preview: string, _variant: string) {
    this.calls.push("downloadStart");
    return "a".repeat(32);
  }
  async downloadCancel(_id: string) {
    this.calls.push("downloadCancel");
  }
  async downloadRetry(_id: string) {
    this.calls.push("downloadRetry");
  }
  async downloadSetAssistantAllowed(enabled: boolean) {
    await this.answer(`downloadSetAssistantAllowed:${enabled}`, undefined);
    const next = {
      ...this.current,
      downloads: { ...this.current.downloads, assistantAllowed: enabled },
    };
    this.push(next);
    return next;
  }
  async downloadChooseFolder() {
    this.calls.push("downloadChooseFolder");
  }
  async downloadOpen(_id: string) {
    this.calls.push("downloadOpen");
  }
  async downloadReveal(_id: string) {
    this.calls.push("downloadReveal");
  }
  current: Snapshot;
  calls: string[] = [];
  failWith: BridgeError | null = null;
  pending = false;
  /** `sendToDevice` 交回的传输标识：面板拿它盯进度。 */
  sentAs: string[] = [];
  /** 最近一次新建或整份替换日程、提醒时交出去的内容。 */
  lastSubmitted: ScheduleContent | ReminderContent | null = null;
  private listeners = new Set<(snapshot: Snapshot) => void>();
  /** 调用过 `hold()` 之后，命令一直不返回，直到 `release()`。 */
  private held = false;
  private waiting: (() => void)[] = [];

  constructor(initial: Snapshot) {
    this.current = initial;
  }

  /**
   * 让之后的命令一直挂着，直到 `release()`。
   * 要确定地观察"进行中"的状态时用它：不要靠零延时定时器和界面的重渲染赛跑，
   * 那个先后顺序在 macOS 与 Windows 上不一样。
   */
  hold() {
    this.held = true;
  }

  release() {
    this.held = false;
    for (const wake of this.waiting.splice(0)) wake();
  }

  private gate(): Promise<void> {
    this.pending = true;
    if (this.held) {
      return new Promise((resolve) => {
        this.waiting.push(() => {
          this.pending = false;
          resolve();
        });
      });
    }
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
  systemNotificationsRefresh() {
    return this.answer("systemNotificationsRefresh", this.current);
  }
  systemNotificationsRead(
    items: import("@assemble/ui/lib/system-notifications").SystemNotificationRead[],
  ) {
    return this.answer(
      `systemNotificationsRead:${items.map((item) => item.id).join(",")}`,
      this.current,
    );
  }

  setSendMenu(enabled: boolean) {
    this.calls.push(`setSendMenu:${enabled}`);
    if (this.failWith) throw this.failWith;
    return Promise.resolve(this.current);
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
  drivePreview(
    fileId: string,
    _signal: AbortSignal,
  ): Promise<import("@assemble/ui/lib/file-preview").PreviewResource> {
    this.calls.push(`drivePreview:${fileId}`);
    return Promise.resolve({
      preview: {
        kind: "unsupported",
        mimeType: null,
        size: 0,
        text: null,
        truncated: false,
      },
      src: null,
    });
  }

  driveDelete(fileId: string) {
    return this.answer(`driveDelete:${fileId}`, this.current);
  }

  driveRename(fileId: string, name: string) {
    return this.answer(`driveRename:${fileId}:${name}`, this.current);
  }

  closeSendPanel() {
    return this.answer("closeSendPanel", undefined);
  }

  openMain() {
    return this.answer("openMain", undefined);
  }

  openPublicMedia() {
    return this.answer("openPublicMedia", undefined);
  }
  openRecordLocation(resource: string, id: string, label: string) {
    return this.answer(
      `openRecordLocation:${resource}:${id}:${label}`,
      undefined,
    );
  }

  updateCheck() {
    this.calls.push("updateCheck");
    return Promise.resolve(this.current.update);
  }

  updateRestart() {
    this.calls.push("updateRestart");
    if (this.failWith) throw this.failWith;
    return Promise.resolve();
  }

  cancelTransfer(transferId: string) {
    return this.answer(`cancelTransfer:${transferId}`, undefined);
  }

  retryTransfer(transferId: string) {
    return this.answer(`retryTransfer:${transferId}`, undefined);
  }

  plannerRefresh() {
    return this.answer("plannerRefresh", this.current);
  }

  scheduleCreate(content: ScheduleContent) {
    this.lastSubmitted = content;
    return this.answer(`scheduleCreate:${content.title}`, this.current);
  }

  scheduleUpdate(id: string, version: number, content: ScheduleContent) {
    this.lastSubmitted = content;
    return this.answer(
      `scheduleUpdate:${id}:${version}:${content.title}`,
      this.current,
    );
  }

  scheduleComplete(id: string, version: number) {
    return this.answer(`scheduleComplete:${id}:${version}`, this.current);
  }

  scheduleCancel(id: string, version: number) {
    return this.answer(`scheduleCancel:${id}:${version}`, this.current);
  }

  scheduleReopen(id: string, version: number) {
    return this.answer(`scheduleReopen:${id}:${version}`, this.current);
  }

  scheduleDelete(id: string, version: number) {
    return this.answer(`scheduleDelete:${id}:${version}`, this.current);
  }

  reminderCreate(content: ReminderContent) {
    this.lastSubmitted = content;
    return this.answer(`reminderCreate:${content.title}`, this.current);
  }

  reminderUpdate(id: string, version: number, content: ReminderContent) {
    this.lastSubmitted = content;
    return this.answer(
      `reminderUpdate:${id}:${version}:${content.title}`,
      this.current,
    );
  }

  reminderDismiss(id: string, version: number) {
    return this.answer(`reminderDismiss:${id}:${version}`, this.current);
  }

  reminderCancel(id: string, version: number) {
    return this.answer(`reminderCancel:${id}:${version}`, this.current);
  }

  reminderDelete(id: string, version: number) {
    return this.answer(`reminderDelete:${id}:${version}`, this.current);
  }

  notificationsRead(ids: string[]) {
    return this.answer(`notificationsRead:${ids.join(",")}`, this.current);
  }

  notificationsReadAll() {
    return this.answer("notificationsReadAll", this.current);
  }

  plannerFocus(notificationId: string | null) {
    return this.answer(`plannerFocus:${notificationId}`, this.current);
  }

  plannerCompose(kind: "reminder" | "schedule" | null) {
    return this.answer(`plannerCompose:${kind}`, this.current);
  }

  setPlannerMuted(muted: boolean) {
    return this.answer(`setPlannerMuted:${muted}`, this.current);
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
