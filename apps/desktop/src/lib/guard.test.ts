// 运行时守卫：哪些状态算认得，哪些必须拦下。

import { describe, expect, test } from "bun:test";

import { isSnapshot } from "@/lib/guard";
import type { Snapshot } from "@/lib/types";

const VALID: Snapshot = {
  service: { target: "production", reachable: true },
  session: { phase: "signedOut" },
  launchAtLogin: "disabled",
  notice: null,
};

describe("状态守卫", () => {
  test("认得合法的状态", () => {
    expect(isSnapshot(VALID)).toBe(true);
    expect(
      isSnapshot({
        ...VALID,
        service: { target: "development", reachable: null },
        session: {
          phase: "signedIn",
          account: { id: "a", username: "u", displayName: "U" },
        },
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
      { ...VALID, session: { phase: "signedIn" } },
      { ...VALID, session: { phase: "signedIn", account: { username: "u" } } },
      { ...VALID, session: { phase: "retired" } },
      { ...VALID, launchAtLogin: "maybe" },
      { ...VALID, notice: { kind: "warning", message: "x" } },
      { ...VALID, notice: { kind: "error" } },
    ];
    for (const value of invalid) {
      expect(isSnapshot(value)).toBe(false);
    }
  });

  test("多余的字段不影响识别，为日后新增字段留出余地", () => {
    expect(isSnapshot({ ...VALID, somethingNew: 1 })).toBe(true);
  });
});
