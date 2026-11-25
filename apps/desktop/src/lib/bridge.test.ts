// 桥接层：命令名、参数名、状态核对与错误收敛。这些必须与 Rust 侧的命令一一对应。

import { beforeEach, describe, expect, mock, test } from "bun:test";

import type { BridgeError as BridgeErrorType } from "@/lib/bridge";
import type { Snapshot } from "@/lib/types";

type InvokeCall = { command: string; args: Record<string, unknown> };

const STATE: Snapshot = {
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

const { SNAPSHOT_EVENT, BridgeError, bridge, run, toBridgeError } =
  await import("@/lib/bridge");
const { messageFor } = await import("@/lib/messages");

describe("桥接", () => {
  beforeEach(() => {
    calls.length = 0;
    listeners.clear();
    respondWith = STATE;
    rejectWith = null;
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
    await bridge.driveDelete("file-1");
    await bridge.closeSendPanel();
    await bridge.openMain();
    await bridge.cancelTransfer("transfer-1");
    await bridge.retryTransfer("transfer-1");
    await bridge.setSendMenu(true);
    await bridge.dismissNotice();
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
      { command: "drive_delete", args: { fileId: "file-1" } },
      { command: "close_send_panel", args: {} },
      { command: "open_main", args: {} },
      { command: "cancel_transfer", args: { transferId: "transfer-1" } },
      { command: "retry_transfer", args: { transferId: "transfer-1" } },
      { command: "set_send_menu", args: { enabled: true } },
      { command: "dismiss_notice", args: {} },
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

describe("用户文案", () => {
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
