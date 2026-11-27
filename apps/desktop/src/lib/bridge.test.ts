// 桥接层：命令名、参数名、状态核对与错误收敛。这些必须与 Rust 侧的命令一一对应。

import { beforeEach, describe, expect, mock, test } from "bun:test";
import type { MediaContent, MediaIdentity } from "@assemble/media";
import type { BridgeError as BridgeErrorType } from "@/lib/bridge";
import { isMemo, isMemoList } from "@/lib/memos";
import type { Snapshot } from "@/lib/types";
import mediaSamples from "../../../../packages/media/contract/snapshots.json";
import memoSamples from "../../contract/memos.json";

type InvokeCall = { command: string; args: Record<string, unknown> };

const STATE: Snapshot = {
  driveTree: null,
  driveTrash: null,
  remote: { sessions: [] },
  systemNotifications: { items: [], available: true },
  downloads: {
    assistantAllowed: true,
    browserTaskId: null,
    folderName: "",
    toolsAvailable: false,
    tasks: [],
  },
  service: { target: "production", reachable: true, stream: "off" },
  session: { phase: "signedOut" },
  devices: [],
  transfers: [],
  draft: null,
  drive: null,
  sendMenu: "unsupported",
  receive: { customFolder: null, displayPath: null },
  launchAtLogin: "disabled",
  notice: null,
  planner: null,
  plannerMuted: false,
  update: {
    state: "idle",
    version: null,
    received: null,
    total: null,
    checkedAt: null,
    error: null,
  },
};

const calls: InvokeCall[] = [];
const listeners = new Map<string, (event: { payload: unknown }) => void>();

let respondWith: unknown = STATE;
let rejectWith: unknown = null;

// 先替换 Tauri 的模块，再加载被测的桥接：它拿到的就是这里的替身。
mock.module("@tauri-apps/api/core", () => ({
  invoke: async <T>(
    command: string,
    args?: Record<string, unknown>,
  ): Promise<T> => {
    calls.push({ command, args: args ?? {} });
    if (rejectWith !== null) throw rejectWith;
    return respondWith as T;
  },
}));

mock.module("@tauri-apps/api/event", () => ({
  listen: async <T>(name: string, handler: (event: { payload: T }) => void) => {
    listeners.set(name, handler as (event: { payload: unknown }) => void);
    return () => {
      listeners.delete(name);
    };
  },
}));

const {
  SNAPSHOT_EVENT,
  BridgeError,
  bridge,
  mediaBridge,
  memoBridge,
  remoteBridge,
  run,
  toBridgeError,
} = await import("@/lib/bridge");
const { messageFor } = await import("@/lib/messages");

describe("桥接", () => {
  test("回收站具名命令核验响应，取消查询仅释放自己的现场，不暴露凭据或路径", async () => {
    const target = { kind: "folder" as const, id: "folder-1", version: 1 };
    const batch = {
      id: "trash-1",
      rootId: target.id,
      kind: target.kind,
      name: "资料<中文>",
      originalLocation: [],
      deletedAt: "2026-11-27T08:00:00Z",
      version: 1,
      fileCount: 0,
      folderCount: 1,
      size: 0,
    };
    const list = { items: [batch], total: 1, page: 1, pageSize: 20 };
    respondWith = list;
    expect(await bridge.driveTrash.list("资料", 1, 20)).toEqual(list);
    const reading = calls.find((call) => call.command === "drive_trash_list");
    expect(reading?.args).toMatchObject({ q: "资料", page: 1, pageSize: 20 });
    expect(calls.at(-1)).toEqual({
      command: "drive_trash_cancel",
      args: { requestId: reading?.args.requestId },
    });
    const query = { parentId: "root", q: "", page: 1, pageSize: 20 };
    const item = {
      id: batch.rootId,
      parentId: null,
      kind: batch.kind,
      name: batch.name,
      size: null,
      version: 2,
      createdAt: batch.deletedAt,
    };
    const detail = {
      batch,
      current: item,
      ancestors: [],
      items: [],
      total: 0,
      page: 1,
      pageSize: 20,
    };
    respondWith = detail;
    expect(await bridge.driveTrash.detail(batch.id, query)).toEqual(detail);
    expect(calls.at(-2)).toMatchObject({
      command: "drive_trash_detail",
      args: { id: batch.id, query },
    });
    expect(calls.at(-1)?.command).toBe("drive_trash_cancel");
    respondWith = {
      target,
      name: batch.name,
      fileCount: 0,
      folderCount: 1,
      size: 0,
      revision: "a".repeat(64),
    };
    await bridge.driveTrash.preview?.(target);
    expect(calls.at(-1)).toEqual({
      command: "drive_trash_preview",
      args: { target },
    });
    respondWith = batch;
    const input = { target, revision: "a".repeat(64), requestId: "stable" };
    await bridge.driveTrash.move?.(input);
    expect(calls.at(-1)).toEqual({
      command: "drive_trash_move",
      args: { input },
    });
    const restored = {
      batchId: batch.id,
      id: batch.rootId,
      kind: batch.kind,
      name: batch.name,
      parentId: null,
      reason: "original",
      renamed: false,
    };
    respondWith = {
      ...restored,
      version: 1,
      revision: "b".repeat(64),
      parentLocation: [],
    };
    await bridge.driveTrash.restorePreview?.(batch.id);
    expect(calls.at(-1)).toEqual({
      command: "drive_trash_restore_preview",
      args: { id: batch.id },
    });
    respondWith = restored;
    const restore = { version: 1, revision: "b".repeat(64) };
    await bridge.driveTrash.restore?.(batch.id, restore);
    expect(calls.at(-1)).toEqual({
      command: "drive_trash_restore",
      args: { id: batch.id, input: restore },
    });
    respondWith = {
      id: batch.id,
      logicalBytesReleased: 0,
      cleanupPending: false,
    };
    await bridge.driveTrash.purge?.(batch.id, 1);
    expect(calls.at(-1)).toEqual({
      command: "drive_trash_purge",
      args: { id: batch.id, version: 1 },
    });
    respondWith = { items: [] };
    await expect(bridge.driveTrash.list("", 1, 20)).rejects.toMatchObject({
      code: "INVALID_STATE",
    });
    const abort = new AbortController();
    abort.abort();
    const before = calls.length;
    await expect(
      bridge.driveTrash.list("", 1, 20, abort.signal),
    ).rejects.toThrow();
    expect(calls).toHaveLength(before);
  });
  test("目录命令只传标识版本和名称；查询取消释放自己的现场", async () => {
    const listing = {
      current: null,
      ancestors: [],
      items: [],
      total: 0,
      page: 1,
      pageSize: 10,
      usedBytes: 0,
      limitBytes: 1024,
    };
    const query = { parentId: "root", q: "资料", page: 1, pageSize: 10 };
    respondWith = listing;
    const abort = new AbortController();
    expect(await bridge.driveTree.list(query, abort.signal, true)).toEqual(
      listing,
    );
    const request = calls.find((call) => call.command === "drive_tree_list");
    expect(request?.args).toMatchObject({ query, remember: true });
    expect(calls.at(-1)).toEqual({
      command: "drive_tree_cancel",
      args: { requestId: request?.args.requestId },
    });
    respondWith = {
      id: "folder-1",
      parentId: null,
      name: "中文<目录>",
      version: 1,
      createdAt: "2026-11-27T01:00:00Z",
      updatedAt: "2026-11-27T01:00:00Z",
    };
    const input = {
      name: "中文<目录>",
      requestId: "stable-key",
      parentId: "root",
    };
    await bridge.driveTree.create?.(input);
    expect(calls.at(-1)).toEqual({
      command: "drive_folder_create",
      args: { input },
    });
    respondWith = { kind: "folder", ...(respondWith as object) };
    await bridge.driveTree.rename?.("folder", "folder-1", {
      name: "新名字",
      version: 1,
    });
    expect(calls.at(-1)?.command).toBe("drive_tree_rename");
    const move = {
      kind: "folder" as const,
      id: "folder-1",
      parentId: "folder-2",
      version: 1,
    };
    await bridge.driveTree.move?.(move);
    expect(calls.at(-1)).toEqual({
      command: "drive_tree_move",
      args: { input: move },
    });
    respondWith = undefined;
    await bridge.driveTree.remove?.("folder", "folder-1", 2);
    expect(calls.at(-1)).toEqual({
      command: "drive_tree_delete",
      args: { kind: "folder", id: "folder-1", version: 2 },
    });
    respondWith = STATE;
    await bridge.driveTree.upload?.("folder-1");
    expect(calls.at(-1)).toEqual({
      command: "drive_tree_upload",
      args: { parentId: "folder-1" },
    });
    const count = calls.length;
    abort.abort();
    await expect(
      bridge.driveTree.list(query, abort.signal),
    ).rejects.toMatchObject({ code: "CANCELLED" });
    expect(calls).toHaveLength(count);
    respondWith = { ...listing, items: [{ id: "bad" }] };
    await expect(bridge.driveTree.list(query)).rejects.toMatchObject({
      code: "INVALID_STATE",
    });
  });
  test("预览只接受本次具名入口，关闭与早到取消都释放原请求", async () => {
    const preview = {
      kind: "image",
      mimeType: "image/png",
      size: 100,
      text: null,
      truncated: false,
    };
    respondWith = {
      generation: 7,
      preview,
      src: "http://assemble-preview.localhost/7/file-1",
    };
    const abort = new AbortController();
    const result = await bridge.drivePreview("file-1", abort.signal);
    const opened = calls.at(-1);
    expect(opened?.command).toBe("drive_preview_open");
    expect(opened?.args.fileId).toBe("file-1");
    abort.abort();
    expect(calls.at(-1)).toEqual({
      command: "drive_preview_close",
      args: { requestId: opened?.args.requestId },
    });
    result.dispose?.();
    const cancelled = new AbortController();
    cancelled.abort();
    const count = calls.length;
    await expect(
      bridge.drivePreview("file-1", cancelled.signal),
    ).rejects.toMatchObject({ code: "CANCELLED" });
    expect(calls.length).toBe(count);
    for (const src of [
      "https://untrusted.test/a",
      "file:///secret",
      "http://assemble-preview.localhost/8/file-1",
      "http://assemble-preview.localhost/7/file-2",
      "http://assemble-preview.localhost/7/file-1?token=bad",
    ]) {
      respondWith = { generation: 7, preview, src };
      await expect(
        bridge.drivePreview("file-1", new AbortController().signal),
      ).rejects.toMatchObject({ code: "INVALID_RESPONSE" });
    }
  });
  test("地点导航只传记录和标签，目标网址由 Rust 重新读取", async () => {
    await bridge.openRecordLocation("schedule", "schedule-1", "上海虹桥站");
    expect(calls.at(-1)).toEqual({
      command: "open_record_location",
      args: { resource: "schedule", id: "schedule-1", label: "上海虹桥站" },
    });
    await memoBridge.openLocation?.("memo-1", "杭州东");
    expect(calls.at(-1)).toEqual({
      command: "open_record_location",
      args: { resource: "memo", id: "memo-1", label: "杭州东" },
    });
  });
  test("远程具名命令核验响应，不向界面暴露凭据或路径", async () => {
    const session = {
      id: "remote-1",
      controllerId: "controller",
      controllerName: "Windows",
      hostId: "host",
      hostName: "Mac",
      phase: "pending",
      createdAt: "2026-11-27T00:00:00Z",
    };
    respondWith = session;
    await remoteBridge.request("host");
    expect(calls.at(-1)).toEqual({
      command: "remote_request",
      args: { hostId: "host" },
    });
    await remoteBridge.approve("remote-1");
    expect(calls.at(-1)?.command).toBe("remote_approve");
    respondWith = { session, frame: null, inputs: [] };
    await remoteBridge.poll("remote-1", 0);
    expect(calls.at(-1)).toEqual({
      command: "remote_poll",
      args: { id: "remote-1", frameAfter: 0 },
    });
    respondWith = {
      session,
      frame: { jpeg: "https://untrusted.test" },
      inputs: [],
    };
    await expect(remoteBridge.poll("remote-1", 0)).rejects.toMatchObject({
      code: "INVALID_STATE",
    });
    await remoteBridge.inputs("remote-1", [
      { sequence: 1, input: { type: "releaseAll" } },
    ]);
    expect(calls.at(-1)?.command).toBe("remote_inputs");
    await remoteBridge.end("remote-1");
    expect(calls.at(-1)?.command).toBe("remote_end");
    await remoteBridge.fullscreen(false);
    expect(calls.at(-1)?.command).toBe("remote_fullscreen");
    respondWith = { state: "active", packets: [] };
    await remoteBridge.audio("remote-1", 7);
    expect(calls.at(-1)).toEqual({
      command: "remote_audio",
      args: { id: "remote-1", after: 7 },
    });
    respondWith = "completed";
    await remoteBridge.clipboard("remote-1", "toHost", true);
    expect(calls.at(-1)).toEqual({
      command: "remote_clipboard",
      args: { id: "remote-1", direction: "toHost", paste: true },
    });
    respondWith = { text: "must stay in Rust" };
    await expect(
      remoteBridge.clipboard("remote-1", "fromHost", false),
    ).rejects.toMatchObject({ code: "INVALID_STATE" });
  });
  test("备忘录契约与具名桥接：复制只传标识，失败响应与取消不交给页面", async () => {
    expect(isMemo(memoSamples.memo)).toBe(true);
    expect(isMemoList(memoSamples.listing)).toBe(true);
    expect(isMemo({ ...memoSamples.memo, version: 0 })).toBe(false);
    expect(isMemoList({ ...memoSamples.listing, page: 0 })).toBe(false);
    respondWith = memoSamples.listing;
    const query = { q: "原文+🙂", page: 2, pageSize: 10 };
    await memoBridge.list(query);
    expect(calls.at(-1)).toEqual({ command: "memo_list", args: { query } });
    respondWith = memoSamples.memo;
    const content = { title: "标题", text: "  原文\n " };
    await memoBridge.get("memo-1");
    expect(calls.at(-1)).toEqual({
      command: "memo_get",
      args: { id: "memo-1" },
    });
    await memoBridge.create(content, "same-request");
    expect(calls.at(-1)).toEqual({
      command: "memo_create",
      args: { input: { content, requestId: "same-request" } },
    });
    await memoBridge.update("memo-1", 1, content);
    expect(calls.at(-1)).toEqual({
      command: "memo_update",
      args: { id: "memo-1", input: { version: 1, content } },
    });
    await memoBridge.remove("memo-1", 1);
    expect(calls.at(-1)).toEqual({
      command: "memo_remove",
      args: { id: "memo-1", version: 1 },
    });
    await memoBridge.copy("memo-1");
    expect(calls.at(-1)).toEqual({
      command: "memo_copy",
      args: { id: "memo-1" },
    });
    respondWith = { invalid: "private diagnostic" };
    await expect(memoBridge.get("memo-1")).rejects.toBeInstanceOf(BridgeError);
    const cancelled = new AbortController();
    cancelled.abort();
    const count = calls.length;
    await expect(memoBridge.copy("memo-1", cancelled.signal)).rejects.toThrow();
    expect(calls).toHaveLength(count);
  });
  test("下载只通过具名命令传来源，文件操作只传任务标识", async () => {
    const preview = {
      id: "p",
      kind: "hls",
      title: "HLS",
      sourceHost: "example.com",
      durationSeconds: null,
      variants: [{ id: "v0", width: null, height: null, bandwidth: null }],
    };
    respondWith = preview;
    const input = {
      kind: "hls" as const,
      url: "https://example.com/a.m3u8",
      pageUrl: null,
    };
    await bridge.downloadPrepare(input);
    await bridge.downloadImportHls(null, null);
    respondWith = "a".repeat(32);
    await bridge.downloadStart("p", "v0");
    await bridge.downloadCancel("task");
    await bridge.downloadRetry("task");
    await bridge.downloadChooseFolder();
    await bridge.downloadOpen("task");
    await bridge.downloadReveal("task");
    expect(calls).toEqual([
      { command: "download_prepare", args: { input } },
      {
        command: "download_import_hls",
        args: { baseUrl: null, pageUrl: null },
      },
      {
        command: "download_start",
        args: { previewId: "p", variantId: "v0", fileIds: [] },
      },
      { command: "download_cancel", args: { taskId: "task" } },
      { command: "download_retry", args: { taskId: "task" } },
      { command: "download_choose_folder", args: {} },
      { command: "download_open", args: { taskId: "task" } },
      { command: "download_reveal", args: { taskId: "task" } },
    ]);
    respondWith = {
      ...preview,
      variants: [{ id: "v0", width: null, height: null, bandwidth: NaN }],
    };
    await expect(bridge.downloadPrepare(input)).rejects.toMatchObject({
      code: "INVALID_RESPONSE",
    });
  });
  beforeEach(() => {
    calls.length = 0;
    listeners.clear();
    respondWith = STATE;
    rejectWith = null;
  });

  test("磁力文件选择与控制只传标识，不接受本机路径", async () => {
    respondWith = "a".repeat(32);
    await bridge.downloadStart("p", "magnet", [0, 2]);
    await bridge.downloadPause("task");
    await bridge.downloadResume("task");
    await bridge.downloadCancelPreparation();
    expect(calls).toEqual([
      {
        command: "download_start",
        args: { previewId: "p", variantId: "magnet", fileIds: [0, 2] },
      },
      { command: "download_pause", args: { taskId: "task" } },
      { command: "download_resume", args: { taskId: "task" } },
      { command: "download_cancel_preparation", args: {} },
    ]);
  });
  test("每条命令使用与 Rust 侧一致的名字与参数", async () => {
    await bridge.snapshot();
    await bridge.signIn();
    await bridge.cancelSignIn();
    await bridge.signOut();
    await bridge.launchAtLogin();
    await bridge.setLaunchAtLogin(true);
    await bridge.renameDevice("device-2", "家里的电脑");
    await bridge.removeDevice("device-2");
    await bridge.chooseReceiveFolder();
    await bridge.resetReceiveFolder();
    await bridge.revealReceived("transfer-1");
    await bridge.startSend();
    // 发起发送的命令交回的是一组标识，不是状态。
    respondWith = ["transfer-1"];
    await bridge.sendToDevice("device-2");
    respondWith = ["transfer-1"];
    await bridge.sendToDrive();
    respondWith = STATE;
    await bridge.driveRefresh();
    await bridge.driveUpload();
    await bridge.driveDownload("file-1");
    await bridge.driveRename("file-1", "合同（新版）.pdf");
    await bridge.downloadSetAssistantAllowed(false);
    await bridge.driveDelete("file-1");
    await bridge.closeSendPanel();
    await bridge.openMain();
    await bridge.openPublicMedia();
    await bridge.cancelTransfer("transfer-1");
    await bridge.retryTransfer("transfer-1");
    await bridge.setSendMenu(true);
    await bridge.dismissNotice();
    await bridge.systemNotificationsRefresh();
    await bridge.systemNotificationsRead([{ id: "system-1", revision: 2 }]);
    await bridge.plannerRefresh();
    await bridge.scheduleCreate({
      title: "火车",
      startAt: "2026-11-25T01:00:00.000Z",
    });
    await bridge.scheduleUpdate("schedule-1", 2, {
      title: "火车",
      startAt: "2026-11-25T02:00:00.000Z",
    });
    await bridge.scheduleComplete("schedule-1", 3);
    await bridge.scheduleCancel("schedule-1", 3);
    await bridge.scheduleReopen("schedule-1", 4);
    await bridge.scheduleDelete("schedule-1", 5);
    await bridge.reminderCreate({
      title: "取快递",
      dueAt: "2026-11-25T01:00:00.000Z",
    });
    await bridge.reminderUpdate("reminder-1", 1, {
      title: "取快递",
      dueAt: "2026-11-25T03:00:00.000Z",
    });
    await bridge.reminderDismiss("reminder-1", 2);
    await bridge.reminderCancel("reminder-1", 2);
    await bridge.reminderDelete("reminder-1", 3);
    await bridge.notificationsRead(["note-1", "note-2"]);
    await bridge.notificationsReadAll();
    await bridge.plannerFocus("note-1");
    await bridge.plannerCompose("reminder");
    await bridge.setPlannerMuted(true);
    // 检查更新交回的是更新这一块状态，不是整份快照。
    respondWith = STATE.update;
    await bridge.updateCheck();
    await bridge.updateRestart();

    expect(calls).toEqual([
      { command: "snapshot", args: {} },
      { command: "sign_in", args: {} },
      { command: "cancel_sign_in", args: {} },
      { command: "sign_out", args: {} },
      { command: "launch_at_login", args: {} },
      { command: "set_launch_at_login", args: { enabled: true } },
      {
        command: "rename_device",
        args: { deviceId: "device-2", name: "家里的电脑" },
      },
      { command: "remove_device", args: { deviceId: "device-2" } },
      { command: "choose_receive_folder", args: {} },
      { command: "reset_receive_folder", args: {} },
      { command: "reveal_received", args: { transferId: "transfer-1" } },
      { command: "start_send", args: {} },
      { command: "send_to_device", args: { deviceId: "device-2" } },
      { command: "send_to_drive", args: {} },
      { command: "drive_refresh", args: {} },
      { command: "drive_upload", args: {} },
      { command: "drive_download", args: { fileId: "file-1" } },
      {
        command: "drive_rename",
        args: { fileId: "file-1", name: "合同（新版）.pdf" },
      },
      { command: "download_set_assistant_allowed", args: { enabled: false } },
      { command: "drive_delete", args: { fileId: "file-1" } },
      { command: "close_send_panel", args: {} },
      { command: "open_main", args: {} },
      { command: "open_public_media", args: {} },
      { command: "cancel_transfer", args: { transferId: "transfer-1" } },
      { command: "retry_transfer", args: { transferId: "transfer-1" } },
      { command: "set_send_menu", args: { enabled: true } },
      { command: "dismiss_notice", args: {} },
      { command: "system_notifications_refresh", args: {} },
      {
        command: "system_notifications_read",
        args: { items: [{ id: "system-1", revision: 2 }] },
      },
      { command: "planner_refresh", args: {} },
      {
        command: "schedule_create",
        args: {
          content: { title: "火车", startAt: "2026-11-25T01:00:00.000Z" },
        },
      },
      {
        command: "schedule_update",
        args: {
          id: "schedule-1",
          version: 2,
          content: { title: "火车", startAt: "2026-11-25T02:00:00.000Z" },
        },
      },
      { command: "schedule_complete", args: { id: "schedule-1", version: 3 } },
      { command: "schedule_cancel", args: { id: "schedule-1", version: 3 } },
      { command: "schedule_reopen", args: { id: "schedule-1", version: 4 } },
      { command: "schedule_delete", args: { id: "schedule-1", version: 5 } },
      {
        command: "reminder_create",
        args: {
          content: { title: "取快递", dueAt: "2026-11-25T01:00:00.000Z" },
        },
      },
      {
        command: "reminder_update",
        args: {
          id: "reminder-1",
          version: 1,
          content: { title: "取快递", dueAt: "2026-11-25T03:00:00.000Z" },
        },
      },
      { command: "reminder_dismiss", args: { id: "reminder-1", version: 2 } },
      { command: "reminder_cancel", args: { id: "reminder-1", version: 2 } },
      { command: "reminder_delete", args: { id: "reminder-1", version: 3 } },
      { command: "notifications_read", args: { ids: ["note-1", "note-2"] } },
      { command: "notifications_read_all", args: {} },
      { command: "planner_focus", args: { notificationId: "note-1" } },
      { command: "planner_compose", args: { compose: "reminder" } },
      { command: "set_planner_muted", args: { muted: true } },
      { command: "update_check", args: {} },
      { command: "update_restart", args: {} },
    ]);
  });

  test("订阅状态事件，并在取消后不再监听", async () => {
    const received: Snapshot[] = [];
    const stop = await bridge.subscribe((snapshot) => {
      received.push(snapshot);
    });

    listeners.get(SNAPSHOT_EVENT)?.({ payload: STATE });
    expect(received).toEqual([STATE]);

    stop();
    expect(listeners.has(SNAPSHOT_EVENT)).toBe(false);
  });

  test("形状不对的状态在交界处被拦下", async () => {
    respondWith = { session: { phase: "signedIn" } };
    const failure = await bridge.snapshot().catch((error: unknown) => error);
    expect(failure).toBeInstanceOf(BridgeError);
    expect((failure as BridgeErrorType).code).toBe("INVALID_STATE");

    // 推送来的状态同样要核对，不会把半成品交给界面。
    const received: Snapshot[] = [];
    await bridge.subscribe((snapshot) => {
      received.push(snapshot);
    });
    expect(() =>
      listeners.get(SNAPSHOT_EVENT)?.({ payload: { phase: "weird" } }),
    ).toThrow(BridgeError);
    expect(received).toEqual([]);
  });

  test("标识列表形状不对时在交界处被拦下", async () => {
    respondWith = { id: "transfer-1" };
    const failure = await bridge
      .sendToDevice("device-2")
      .catch((error: unknown) => error);
    expect(failure).toBeInstanceOf(BridgeError);
    expect((failure as BridgeErrorType).code).toBe("INVALID_STATE");

    respondWith = ["transfer-1", 7];
    await expect(bridge.sendToDevice("device-2")).rejects.toBeInstanceOf(
      BridgeError,
    );
  });

  test("Rust 的拒绝被收敛成带稳定错误码的错误", () => {
    const structured = toBridgeError({
      code: "UNREACHABLE",
      message: "connect: ECONNREFUSED 1.2.3.4",
    });
    expect(structured).toBeInstanceOf(BridgeError);
    expect(structured.code).toBe("UNREACHABLE");

    const textual = toBridgeError("command set_launch_at_login not found");
    expect(textual.code).toBe("UNKNOWN");

    expect(toBridgeError(structured)).toBe(structured);
    expect(toBridgeError(null).code).toBe("UNKNOWN");
    expect(toBridgeError({ code: 42 }).code).toBe("UNKNOWN");
  });

  test("命令失败时抛出的是收敛后的错误", async () => {
    rejectWith = { code: "SYSTEM_REJECTED", message: "os error 5" };
    const failure = await run(() => bridge.launchAtLogin()).catch(
      (error: unknown) => error,
    );
    expect(failure).toBeInstanceOf(BridgeError);
    expect((failure as BridgeErrorType).code).toBe("SYSTEM_REJECTED");

    rejectWith = null;
    respondWith = "disabled";
    await expect(run(() => bridge.launchAtLogin())).resolves.toBe("disabled");
  });
});

describe("影视具名桥接", () => {
  beforeEach(() => {
    calls.length = 0;
    rejectWith = null;
  });
  const content: MediaContent = {
    published: false,
    mediaType: "movie",
    title: "测试电影",
    originalTitle: "",
    year: null,
    releaseDate: null,
    overview: "",
    genres: [],
    episodeCount: null,
    status: "planned",
    watchedAt: null,
    review: null,
    notes: null,
    magnetUri: null,
  };
  const identity: MediaIdentity = {
    provider: "tmdb",
    mediaType: "movie",
    externalId: "42",
  };
  test("查询、维护、任务、图片与复制都使用对应具名命令", async () => {
    const client = mediaBridge.client("private");
    const scenarios: {
      command: string;
      args: Record<string, unknown>;
      value: unknown;
      perform: () => Promise<unknown>;
    }[] = [
      {
        command: "media_capabilities",
        args: {},
        value: mediaSamples.capabilities,
        perform: () => client.capabilities(),
      },
      {
        command: "media_list",
        args: { scope: "private", query: { status: "watching" } },
        value: mediaSamples.listing,
        perform: () => client.list({ status: "watching" }),
      },
      {
        command: "media_get",
        args: { scope: "private", id: "entry-1" },
        value: mediaSamples.entries[0],
        perform: () => client.get("entry-1"),
      },
      {
        command: "media_create",
        args: { input: { content, requestId: "request-1" } },
        value: mediaSamples.entries[0],
        perform: () => client.create(content, "request-1"),
      },
      {
        command: "media_update",
        args: { id: "entry-1", input: { version: 1, content } },
        value: mediaSamples.entries[0],
        perform: () => client.update("entry-1", 1, content),
      },
      {
        command: "media_remove",
        args: { id: "entry-1", version: 1 },
        value: null,
        perform: () => client.remove("entry-1", 1),
      },
      {
        command: "media_search",
        args: { provider: "tmdb", mediaType: "movie", q: "电影" },
        value: [mediaSamples.candidate],
        perform: () => client.search("tmdb", "movie", "电影"),
      },
      {
        command: "media_catalog",
        args: { identity },
        value: mediaSamples.candidate,
        perform: () => client.catalog(identity),
      },
      {
        command: "media_import",
        args: { input: { identity, requestId: "request-1" } },
        value: mediaSamples.job,
        perform: () => client.import(identity, "request-1"),
      },
      {
        command: "media_job",
        args: { id: "job-1" },
        value: mediaSamples.job,
        perform: () => client.job("job-1"),
      },
      {
        command: "media_gallery",
        args: { scope: "private", id: "entry-1" },
        value: mediaSamples.gallery,
        perform: () => client.gallery("entry-1"),
      },
      {
        command: "media_sync_gallery",
        args: { id: "entry-1", input: { version: 1, requestId: "request-1" } },
        value: mediaSamples.job,
        perform: () => client.syncGallery("entry-1", 1, "request-1"),
      },
      {
        command: "media_select_poster",
        args: { id: "entry-1", input: { version: 1, assetId: "asset-1" } },
        value: mediaSamples.entries[0],
        perform: () => client.selectPoster("entry-1", 1, "asset-1"),
      },
      {
        command: "media_actor",
        args: {
          identity: { provider: "tmdb", externalId: "1" },
          mediaType: "tv",
          page: 2,
          pageSize: 20,
        },
        value: {
          actor: {
            identity: { provider: "tmdb", externalId: "1" },
            name: "演员",
            photo: null,
          },
          items: [],
          total: 44,
          page: 2,
          pageSize: 20,
        },
        perform: () =>
          client.actor({ provider: "tmdb", externalId: "1" }, "tv", {
            page: 2,
            pageSize: 20,
          }),
      },
      {
        command: "media_image",
        args: { assetId: "asset-1", variant: "200" },
        value: "data:image/png;base64,YQ==",
        perform: () =>
          mediaBridge.image(
            {
              assetId: "asset-1",
              width: 1,
              height: 1,
              contentType: "image/png",
            },
            "200",
          ),
      },
      {
        command: "media_copy_magnet",
        args: { scope: "shared", id: "entry-1" },
        value: null,
        perform: () => mediaBridge.copyMagnet("shared", "entry-1"),
      },
    ];
    for (const scenario of scenarios) {
      respondWith = scenario.value;
      await scenario.perform();
      expect(calls.at(-1)).toEqual({
        command: scenario.command,
        args: scenario.args,
      });
    }
  });
  test("共享维护不会发命令，坏状态和非图片数据在交界拦截", async () => {
    expect(() =>
      mediaBridge.client("shared").create(content, "request-1"),
    ).toThrow("shared media is maintained");
    expect(calls).toHaveLength(0);
    respondWith = { items: [] };
    await expect(mediaBridge.client("private").list()).rejects.toMatchObject({
      code: "INVALID_STATE",
    });
    respondWith = "https://third-party.example/poster.jpg";
    await expect(
      mediaBridge.image(
        { assetId: "asset-1", width: 1, height: 1, contentType: "image/png" },
        "200",
      ),
    ).rejects.toMatchObject({ code: "INVALID_STATE" });
  });
  test("已取消的页面查询不会再进入IPC", async () => {
    const cancel = new AbortController();
    cancel.abort();
    await expect(
      mediaBridge.client("private").list({}, cancel.signal),
    ).rejects.toThrow();
    expect(calls).toHaveLength(0);
  });
});

describe("用户文案", () => {
  test("云盘暂停继续使用具名命令和资源标识", async () => {
    respondWith = undefined;
    await bridge.pauseTransfer("out-one");
    await bridge.resumeTransfer("out-one");
    expect(calls).toEqual([
      { command: "pause_transfer", args: { transferId: "out-one" } },
      { command: "resume_transfer", args: { transferId: "out-one" } },
    ]);
  });
  test("只按错误码给出固定文案，不回显技术细节", () => {
    const error = new BridgeError(
      "UNREACHABLE",
      "connect tcp 10.0.0.1:3300: connection refused",
    );
    expect(messageFor(error)).toBe("暂时连不上服务，请检查网络后重试。");
    expect(messageFor(error).includes("10.0.0.1")).toBe(false);
  });

  test("认不出的错误码使用通用提示", () => {
    expect(messageFor(new BridgeError("SOMETHING_NEW", "raw detail"))).toBe(
      "操作没有完成，请稍后重试。",
    );
  });
});
