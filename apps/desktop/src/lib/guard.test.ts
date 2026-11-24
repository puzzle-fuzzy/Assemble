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
  receive: { customFolder: null },
  launchAtLogin: "disabled",
  notice: null,
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
      }),
    ).toBe(true);
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
