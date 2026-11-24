// 运行时守卫：哪些状态算认得，哪些必须拦下。

import { describe, expect, test } from "bun:test";

import { isSnapshot } from "@/lib/guard";
import type { Snapshot, TransferView } from "@/lib/types";

const ACCOUNT = { id: "a", username: "u", displayName: "U" };
const DEVICE = { id: "d", name: "MacBook", platform: "macos", online: true };

const VALID: Snapshot = {
  service: { target: "production", reachable: true, stream: "off" },
  session: { phase: "signedOut" },
  devices: [],
  transfers: [],
  draft: null,
  drive: null,
  sendMenu: "unsupported",
  receive: { customFolder: null },
  launchAtLogin: "disabled",
  notice: null,
  planner: null,
  plannerMuted: false,
};

/** 一份日程名单：一条日程、一条提醒、一条没读的通知，定位在其中一条上。 */
const PLANNER = {
  notifications: [
    {
      id: "n1",
      title: "周会",
      body: null,
      scheduleId: "s1",
      reminderId: null,
      raisedAt: "2026-11-25T01:00:00.000Z",
    },
  ],
  schedules: [
    {
      id: "s1",
      version: 2,
      createdBy: "ZCode 的助手",
      title: "周会",
      location: { label: "三楼", address: "小会议室" },
      startAt: "2026-11-25T01:00:00.000Z",
      endAt: null,
      repeat: "daily",
      repeatUntil: null,
      remindBefore: 30,
      notes: null,
      occurrences: ["2026-11-25T01:00:00.000Z"],
    },
  ],
  reminders: [
    {
      id: "r1",
      version: 1,
      status: "scheduled",
      createdBy: "本机",
      title: "取快递",
      dueAt: "2026-11-25T03:00:00.000Z",
      notes: "楼下",
    },
  ],
  focus: "n1",
  compose: null,
};

/** 一条已经收好的传输。 */
const TRANSFER: TransferView = {
  id: "t1",
  direction: "in",
  peer: "工作电脑",
  name: "合同.pdf",
  size: 1024,
  phase: "complete",
  progress: 1,
  savedAs: null,
  failure: null,
  retryable: false,
  target: "device",
};

function transfer(overrides: Partial<TransferView>): TransferView {
  return { ...TRANSFER, ...overrides };
}

/** 故意不合法的传输记录：每个都有一处取值不对。 */
const BROKEN_TRANSFERS: unknown[] = [
  { id: "t2", name: "a" },
  { ...TRANSFER, direction: "sideways" },
  { ...TRANSFER, phase: "paused" },
  { ...TRANSFER, progress: "45%" },
  { ...TRANSFER, savedAs: 7 },
  { ...TRANSFER, target: "somewhere" },
  { ...TRANSFER, failure: "alien" },
];

describe("状态守卫", () => {
  test("认得合法的状态", () => {
    expect(isSnapshot(VALID)).toBe(true);
    expect(
      isSnapshot({
        ...VALID,
        service: {
          target: "development",
          reachable: null,
          stream: "connected",
        },
        session: { phase: "signedIn", account: ACCOUNT, device: DEVICE },
        devices: [DEVICE, { ...DEVICE, id: "d2", online: false }],
        launchAtLogin: "unknown",
        notice: { kind: "info", message: "ok" },
        planner: PLANNER,
      }),
    ).toBe(true);
  });

  test("认得日程名单，拦下形状不对的", () => {
    expect(isSnapshot({ ...VALID, planner: PLANNER })).toBe(true);
    expect(isSnapshot({ ...VALID, planner: { ...PLANNER, focus: null } })).toBe(
      true,
    );
    // 从托盘的"新建提醒…"进来：带着要摆出的表单。
    expect(
      isSnapshot({
        ...VALID,
        planner: { ...PLANNER, compose: "reminder" },
      }),
    ).toBe(true);
    const at = { ...PLANNER, notifications: [] };
    const invalid: unknown[] = [
      { ...VALID, planner: {} },
      { ...VALID, plannerMuted: null },
      { ...VALID, planner: { ...at, compose: "note" } },
      { ...VALID, planner: { ...at, reminders: [{ id: "r1" }] } },
      {
        ...VALID,
        // 名单里不会有"办完了"的提醒：那种不该再摆给人看。
        planner: {
          ...at,
          reminders: [{ ...PLANNER.reminders[0], status: "dismissed" }],
        },
      },
      {
        ...VALID,
        planner: {
          ...at,
          schedules: [{ ...PLANNER.schedules[0], repeat: "hourly" }],
        },
      },
      {
        ...VALID,
        planner: {
          ...at,
          schedules: [
            {
              ...PLANNER.schedules[0],
              // 场次的时刻是一串时间，不是一个字符串。
              occurrences: "2026-11-25T01:00:00.000Z",
            },
          ],
        },
      },
      {
        ...VALID,
        planner: {
          ...at,
          notifications: [{ ...PLANNER.notifications[0], scheduleId: 42 }],
        },
      },
    ];
    for (const value of invalid) {
      expect(isSnapshot(value)).toBe(false);
    }
  });

  test("拦下缺字段或取值不对的状态", () => {
    const invalid: unknown[] = [
      null,
      "signedOut",
      [],
      {},
      { ...VALID, service: { target: "production" } },
      { ...VALID, service: { target: "staging", reachable: true } },
      { ...VALID, service: { target: "production", reachable: true } },
      {
        ...VALID,
        service: { target: "production", reachable: true, stream: "paused" },
      },
      { ...VALID, devices: undefined },
      { ...VALID, devices: {} },
      { ...VALID, devices: [DEVICE, { id: "d2", name: "PC" }] },
      { ...VALID, devices: [{ ...DEVICE, online: "yes" }] },
      { ...VALID, session: { phase: "signedIn" } },
      { ...VALID, session: { phase: "signedIn", account: ACCOUNT } },
      {
        ...VALID,
        session: {
          phase: "signedIn",
          account: { username: "u" },
          device: DEVICE,
        },
      },
      {
        ...VALID,
        session: {
          phase: "signedIn",
          account: ACCOUNT,
          device: { ...DEVICE, platform: "darwin" },
        },
      },
      {
        ...VALID,
        session: { phase: "signedIn", account: ACCOUNT, device: { id: "d" } },
      },
      { ...VALID, session: { phase: "retired" } },
      { ...VALID, transfers: undefined },
      { ...VALID, transfers: {} },
      { ...VALID, transfers: [transfer({}), { id: "t2", name: "a" }] },
      ...BROKEN_TRANSFERS.map((broken) => ({ ...VALID, transfers: [broken] })),
      { ...VALID, receive: undefined },
      { ...VALID, receive: {} },
      { ...VALID, receive: { customFolder: 42 } },
      { ...VALID, drive: {} },
      { ...VALID, drive: { items: [], usedBytes: 1 } },
      {
        ...VALID,
        drive: { items: [{ id: "f1" }], usedBytes: 1, limitBytes: 2 },
      },
      { ...VALID, launchAtLogin: "maybe" },
      { ...VALID, sendMenu: "maybe" },
      { ...VALID, sendMenu: null },
      { ...VALID, notice: { kind: "warning", message: "x" } },
      { ...VALID, notice: { kind: "error" } },
    ];
    for (const value of invalid) {
      expect(isSnapshot(value)).toBe(false);
    }
  });

  test("认得各个阶段的传输与用户选的接收文件夹", () => {
    const transfers = [
      transfer({ phase: "queued", progress: 0 }),
      transfer({ phase: "inProgress", progress: 0.45 }),
      TRANSFER,
      transfer({ phase: "failed", progress: 0, failure: "diskFull" }),
      transfer({ phase: "cancelled", progress: 0 }),
      transfer({ phase: "expired", progress: 0 }),
      transfer({ savedAs: "合同 (2).pdf" }),
    ];
    expect(
      isSnapshot({
        ...VALID,
        session: { phase: "signedIn", account: ACCOUNT, device: DEVICE },
        transfers,
        receive: { customFolder: "工作资料" },
      }),
    ).toBe(true);
    // 传输记录只带标识与文件名：样本里从来不出现本机路径。
    expect(JSON.stringify(transfers)).not.toContain("/");
  });

  test("多余的字段不影响识别，为日后新增字段留出余地", () => {
    expect(isSnapshot({ ...VALID, somethingNew: 1 })).toBe(true);
  });
});
