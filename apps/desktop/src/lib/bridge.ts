// 与 Rust 壳通信的唯一通道。
//
// 界面没有通用的网络或文件能力：只能调用这里列出的几个具名命令，
// 并接收 Rust 主动推来的状态。命令名与参数名必须和 `src-tauri/src/commands.rs` 一致。

import type { ImageLoader, MediaClient, MediaScope } from "@assemble/media";
import {
  isMediaActorCredits,
  isMediaCandidate,
  isMediaCapabilities,
  isMediaEntry,
  isMediaGallery,
  isMediaJob,
  isMediaList,
} from "@assemble/media/guard";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { isSnapshot, isUpdateView } from "@/lib/guard";
import { isMemo, isMemoList, type MemoBridge } from "@/lib/memos";
import {
  isAudioPlayback,
  isClipboardStatus,
  isRemotePoll,
  isRemoteSession,
  type RemoteBridge,
} from "@/lib/remote";
import type {
  LaunchState,
  ReminderContent,
  ScheduleContent,
  Snapshot,
  UpdateView,
} from "@/lib/types";
import {
  type DownloadInput,
  type DownloadPreview,
  isDownloadPreview,
} from "./downloads";

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
  systemNotificationsRefresh(): Promise<Snapshot>;
  systemNotificationsRead(
    items: import("@assemble/ui/lib/system-notifications").SystemNotificationRead[],
  ): Promise<Snapshot>;
  downloadPrepare(input: DownloadInput): Promise<DownloadPreview>;
  downloadImportHls(
    baseUrl: string | null,
    pageUrl: string | null,
  ): Promise<DownloadPreview | null>;
  downloadStart(previewId: string, variantId: string): Promise<string>;
  downloadCancel(taskId: string): Promise<void>;
  downloadRetry(taskId: string): Promise<void>;
  downloadSetAssistantAllowed(enabled: boolean): Promise<Snapshot>;
  downloadChooseFolder(): Promise<void>;
  downloadOpen(taskId: string): Promise<void>;
  downloadReveal(taskId: string): Promise<void>;
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
  /** 挑要发送的文件，然后打开发送面板。位置由系统选择器直接交给 Rust，不经过界面。 */
  startSend(): Promise<Snapshot>;
  /** 把挑好的文件发给一台设备。返回每一条传输的标识，面板据此盯着进度。 */
  sendToDevice(deviceId: string): Promise<string[]>;
  /** 把挑好的文件存进云盘。返回每一条传输的标识。 */
  sendToDrive(): Promise<string[]>;
  /** 关掉发送面板。挑好却没发的文件不再留着。 */
  closeSendPanel(): Promise<void>;
  /** 向服务端要一份最新的云盘名单。 */
  driveRefresh(): Promise<Snapshot>;
  /** 挑几个文件存进云盘。位置由系统选择器直接交给 Rust，不经过界面。 */
  driveUpload(): Promise<Snapshot>;
  /** 把云盘里的一个文件取回到接收位置。 */
  driveDownload(fileId: string): Promise<Snapshot>;
  /** 给云盘文件改名；名字不是本机路径。 */
  driveRename(fileId: string, name: string): Promise<Snapshot>;
  /** 从云盘里删掉一个文件。 */
  driveDelete(fileId: string): Promise<Snapshot>;
  /** 把主窗口叫到前面。发送面板里"请先登录"时用。 */
  openMain(): Promise<void>;
  openPublicMedia(): Promise<void>;
  /** 取消一条传输：还没传完的停下，已经到服务端的撤回。 */
  cancelTransfer(transferId: string): Promise<void>;
  /** 再发一次。只有"文件根本没到服务端"的失败才有用。 */
  retryTransfer(transferId: string): Promise<void>;
  /** 在文件的右键菜单里加入（或去掉）"用 Assemble 发送"。只有 Windows 上的打包运行支持。 */
  setSendMenu(enabled: boolean): Promise<Snapshot>;
  /** 提示看过了，收起来。 */
  dismissNotice(): Promise<Snapshot>;
  /** 向服务端要一份最新的日程名单。 */
  plannerRefresh(): Promise<Snapshot>;
  /** 新建一条日程。每次提交带一个新的幂等键，重试不会建出两条。 */
  scheduleCreate(content: ScheduleContent): Promise<Snapshot>;
  /** 整份替换一条日程的内容。 */
  scheduleUpdate(
    id: string,
    version: number,
    content: ScheduleContent,
  ): Promise<Snapshot>;
  scheduleComplete(id: string, version: number): Promise<Snapshot>;
  scheduleCancel(id: string, version: number): Promise<Snapshot>;
  /** 完成或取消之后改回进行中。 */
  scheduleReopen(id: string, version: number): Promise<Snapshot>;
  scheduleDelete(id: string, version: number): Promise<Snapshot>;
  reminderCreate(content: ReminderContent): Promise<Snapshot>;
  reminderUpdate(
    id: string,
    version: number,
    content: ReminderContent,
  ): Promise<Snapshot>;
  /** 一条提醒办完了，或者到点之后看过了。 */
  reminderDismiss(id: string, version: number): Promise<Snapshot>;
  reminderCancel(id: string, version: number): Promise<Snapshot>;
  reminderDelete(id: string, version: number): Promise<Snapshot>;
  /** 把几条通知标成已读。 */
  notificationsRead(ids: string[]): Promise<Snapshot>;
  notificationsReadAll(): Promise<Snapshot>;
  /** 点开了一条通知（或者看完收起）：主窗口定位到它。 */
  plannerFocus(notificationId: string | null): Promise<Snapshot>;
  /** 摆出一张新建表单，或者把它收起。 */
  plannerCompose(kind: "reminder" | "schedule" | null): Promise<Snapshot>;
  /** 这台电脑到点时要不要弹提醒。 */
  setPlannerMuted(muted: boolean): Promise<Snapshot>;
  /** 现在就查一次更新；有新版的话等它下载完才返回。不自动更新的构建原样回它的状态。 */
  updateCheck(): Promise<UpdateView>;
  /** 换上已经下载好的新版并重新打开窗口。 */
  updateRestart(): Promise<void>;
  /** 订阅状态变化，返回取消订阅的函数。 */
  subscribe(listener: (snapshot: Snapshot) => void): Promise<UnlistenFn>;
}

export const bridge: Bridge = {
  downloadPrepare: async (input) =>
    requireDownloadPreview(
      await invoke<unknown>("download_prepare", { input }),
    ),
  downloadImportHls: async (baseUrl, pageUrl) => {
    const value = await invoke<unknown>("download_import_hls", {
      baseUrl,
      pageUrl,
    });
    return value === null ? null : requireDownloadPreview(value);
  },
  downloadStart: async (previewId, variantId) => {
    const value = await invoke<unknown>("download_start", {
      previewId,
      variantId,
    });
    if (typeof value !== "string" || !/^[a-f0-9]{32}$/.test(value))
      throw new BridgeError("INVALID_RESPONSE", "invalid download identifier");
    return value;
  },
  downloadCancel: (taskId) => invoke<void>("download_cancel", { taskId }),
  downloadRetry: (taskId) => invoke<void>("download_retry", { taskId }),
  downloadSetAssistantAllowed: (enabled) =>
    checked(() =>
      invoke<unknown>("download_set_assistant_allowed", { enabled }),
    ),
  downloadChooseFolder: () => invoke<void>("download_choose_folder"),
  downloadOpen: (taskId) => invoke<void>("download_open", { taskId }),
  downloadReveal: (taskId) => invoke<void>("download_reveal", { taskId }),
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
  startSend: () => checked(() => invoke<unknown>("start_send")),
  sendToDevice: (deviceId) =>
    ids(() => invoke<unknown>("send_to_device", { deviceId })),
  sendToDrive: () => ids(() => invoke<unknown>("send_to_drive")),
  driveRefresh: () => checked(() => invoke<unknown>("drive_refresh")),
  driveUpload: () => checked(() => invoke<unknown>("drive_upload")),
  driveDownload: (fileId) =>
    checked(() => invoke<unknown>("drive_download", { fileId })),
  driveRename: (fileId, name) =>
    checked(() => invoke<unknown>("drive_rename", { fileId, name })),
  driveDelete: (fileId) =>
    checked(() => invoke<unknown>("drive_delete", { fileId })),
  closeSendPanel: () => invoke<void>("close_send_panel"),
  openMain: () => invoke<void>("open_main"),
  openPublicMedia: () => invoke<void>("open_public_media"),
  cancelTransfer: (transferId) =>
    invoke<void>("cancel_transfer", { transferId }),
  retryTransfer: (transferId) => invoke<void>("retry_transfer", { transferId }),
  setSendMenu: (enabled) =>
    checked(() => invoke<unknown>("set_send_menu", { enabled })),
  dismissNotice: () => checked(() => invoke<unknown>("dismiss_notice")),
  systemNotificationsRefresh: () =>
    checked(() => invoke<unknown>("system_notifications_refresh")),
  systemNotificationsRead: (items) =>
    checked(() => invoke<unknown>("system_notifications_read", { items })),
  plannerRefresh: () => checked(() => invoke<unknown>("planner_refresh")),
  scheduleCreate: (content) =>
    checked(() => invoke<unknown>("schedule_create", { content })),
  scheduleUpdate: (id, version, content) =>
    checked(() => invoke<unknown>("schedule_update", { id, version, content })),
  scheduleComplete: (id, version) =>
    checked(() => invoke<unknown>("schedule_complete", { id, version })),
  scheduleCancel: (id, version) =>
    checked(() => invoke<unknown>("schedule_cancel", { id, version })),
  scheduleReopen: (id, version) =>
    checked(() => invoke<unknown>("schedule_reopen", { id, version })),
  scheduleDelete: (id, version) =>
    checked(() => invoke<unknown>("schedule_delete", { id, version })),
  reminderCreate: (content) =>
    checked(() => invoke<unknown>("reminder_create", { content })),
  reminderUpdate: (id, version, content) =>
    checked(() => invoke<unknown>("reminder_update", { id, version, content })),
  reminderDismiss: (id, version) =>
    checked(() => invoke<unknown>("reminder_dismiss", { id, version })),
  reminderCancel: (id, version) =>
    checked(() => invoke<unknown>("reminder_cancel", { id, version })),
  reminderDelete: (id, version) =>
    checked(() => invoke<unknown>("reminder_delete", { id, version })),
  notificationsRead: (ids) =>
    checked(() => invoke<unknown>("notifications_read", { ids })),
  notificationsReadAll: () =>
    checked(() => invoke<unknown>("notifications_read_all")),
  plannerFocus: (notificationId) =>
    checked(() => invoke<unknown>("planner_focus", { notificationId })),
  plannerCompose: (kind) =>
    checked(() => invoke<unknown>("planner_compose", { compose: kind })),
  setPlannerMuted: (muted) =>
    checked(() => invoke<unknown>("set_planner_muted", { muted })),
  updateCheck: () => updateState(() => invoke<unknown>("update_check")),
  updateRestart: () => invoke<void>("update_restart"),
  subscribe: (listener) =>
    listen<unknown>(SNAPSHOT_EVENT, (event) => {
      listener(requireSnapshot(event.payload));
    }),
};

function requireDownloadPreview(value: unknown): DownloadPreview {
  if (!isDownloadPreview(value))
    throw new BridgeError("INVALID_RESPONSE", "invalid download preview");
  return value;
}

/** 取回一份状态，并在交界处核对它的形状。 */
async function checked(load: () => Promise<unknown>): Promise<Snapshot> {
  return requireSnapshot(await run(load));
}

/** 取回自动更新的状态，并在交界处核对它的形状。 */
async function updateState(load: () => Promise<unknown>): Promise<UpdateView> {
  const value = await run(load);
  if (!isUpdateView(value))
    throw new BridgeError("INVALID_RESPONSE", "invalid update state");
  return value;
}

/** 取回一组标识，并在交界处核对它的形状。 */
async function ids(load: () => Promise<unknown>): Promise<string[]> {
  const value = await run(load);
  if (
    Array.isArray(value) &&
    value.every((id) => typeof id === "string" && id.length > 0)
  ) {
    return value as string[];
  }
  throw new BridgeError(
    "INVALID_STATE",
    "the identifiers sent by the desktop shell have an unknown shape",
  );
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

export interface MediaBridge {
  client(scope: MediaScope): MediaClient;
  image: ImageLoader;
  copyMagnet(scope: MediaScope, id: string): Promise<void>;
}

/** 复制只传标识，Rust 重新核验账户并读取原文后写系统剪贴板。 */
export const memoBridge: MemoBridge = {
  list: (query, signal) =>
    mediaInvoke("memo_list", { query }, isMemoList, signal),
  get: (id, signal) => mediaInvoke("memo_get", { id }, isMemo, signal),
  create: (content, requestId, signal) =>
    mediaInvoke(
      "memo_create",
      { input: { content, requestId } },
      isMemo,
      signal,
    ),
  update: (id, version, content, signal) =>
    mediaInvoke(
      "memo_update",
      { id, input: { version, content } },
      isMemo,
      signal,
    ),
  remove: async (id, version, signal) => {
    signal?.throwIfAborted();
    await run(() => invoke<void>("memo_remove", { id, version }));
    signal?.throwIfAborted();
  },
  copy: async (id, signal) => {
    signal?.throwIfAborted();
    await run(() => invoke<void>("memo_copy", { id }));
    signal?.throwIfAborted();
  },
};

export const remoteBridge: RemoteBridge = {
  audio: (id, after) =>
    mediaInvoke("remote_audio", { id, after }, isAudioPlayback),
  clipboard: (id, direction, paste) =>
    mediaInvoke(
      "remote_clipboard",
      { id, direction, paste },
      isClipboardStatus,
    ),
  request: (hostId) =>
    mediaInvoke("remote_request", { hostId }, isRemoteSession),
  approve: (id) => mediaInvoke("remote_approve", { id }, isRemoteSession),
  end: (id) => run(() => invoke<void>("remote_end", { id })),
  poll: (id, frameAfter) =>
    mediaInvoke("remote_poll", { id, frameAfter }, isRemotePoll),
  inputs: (id, events) =>
    run(() => invoke<void>("remote_inputs", { id, events })),
  fullscreen: (enabled) =>
    run(() => invoke<void>("remote_fullscreen", { enabled })),
};

async function mediaInvoke<T>(
  command: string,
  args: Record<string, unknown>,
  guard: (value: unknown) => value is T,
  signal?: AbortSignal,
): Promise<T> {
  signal?.throwIfAborted();
  const value = await run(() => invoke<unknown>(command, args));
  signal?.throwIfAborted();
  if (!guard(value))
    throw new BridgeError(
      "INVALID_STATE",
      "the command response has an unknown shape",
    );
  return value;
}
function requirePrivate(scope: MediaScope): void {
  if (scope !== "private")
    throw new BridgeError(
      "FORBIDDEN",
      "shared media is maintained in the admin app",
    );
}

/** 影视同样只走具名命令；界面既不能指定网络地址，也拿不到本机图片路径。 */
export const mediaBridge: MediaBridge = {
  client: (scope) => ({
    scope,
    capabilities: (signal) =>
      mediaInvoke("media_capabilities", {}, isMediaCapabilities, signal),
    list: (query = {}, signal) =>
      mediaInvoke("media_list", { scope, query }, isMediaList, signal),
    get: (id, signal) =>
      mediaInvoke("media_get", { scope, id }, isMediaEntry, signal),
    create: (content, requestId, signal) => {
      requirePrivate(scope);
      return mediaInvoke(
        "media_create",
        { input: { content, requestId } },
        isMediaEntry,
        signal,
      );
    },
    update: (id, version, content, signal) => {
      requirePrivate(scope);
      return mediaInvoke(
        "media_update",
        { id, input: { version, content } },
        isMediaEntry,
        signal,
      );
    },
    remove: async (id, version, signal) => {
      requirePrivate(scope);
      signal?.throwIfAborted();
      await run(() => invoke<void>("media_remove", { id, version }));
      signal?.throwIfAborted();
    },
    search: (provider, mediaType, q, signal) =>
      mediaInvoke(
        "media_search",
        { provider, mediaType, q },
        (value): value is import("@assemble/media").MediaCandidate[] =>
          Array.isArray(value) && value.every(isMediaCandidate),
        signal,
      ),
    catalog: (identity, signal) =>
      mediaInvoke("media_catalog", { identity }, isMediaCandidate, signal),
    actor: (identity, mediaType, query = {}, signal) =>
      mediaInvoke(
        "media_actor",
        {
          identity,
          mediaType,
          page: query.page ?? 1,
          pageSize: query.pageSize ?? 24,
        },
        isMediaActorCredits,
        signal,
      ),
    import: (identity, requestId, signal) => {
      requirePrivate(scope);
      return mediaInvoke(
        "media_import",
        { input: { identity, requestId } },
        isMediaJob,
        signal,
      );
    },
    job: (id, signal) => {
      requirePrivate(scope);
      return mediaInvoke("media_job", { id }, isMediaJob, signal);
    },
    gallery: (id, signal) =>
      mediaInvoke("media_gallery", { scope, id }, isMediaGallery, signal),
    syncGallery: (id, version, requestId, signal) => {
      requirePrivate(scope);
      return mediaInvoke(
        "media_sync_gallery",
        { id, input: { version, requestId } },
        isMediaJob,
        signal,
      );
    },
    selectPoster: (id, version, assetId, signal) => {
      requirePrivate(scope);
      return mediaInvoke(
        "media_select_poster",
        { id, input: { version, assetId } },
        isMediaEntry,
        signal,
      );
    },
  }),
  image: (image, variant, signal) =>
    mediaInvoke(
      "media_image",
      { assetId: image.assetId, variant },
      (value): value is string =>
        typeof value === "string" &&
        /^data:image\/(?:jpeg|png|webp);base64,[A-Za-z0-9+/]+={0,2}$/.test(
          value,
        ),
      signal,
    ),
  copyMagnet: (scope, id) =>
    run(() => invoke<void>("media_copy_magnet", { scope, id })),
};
