// 契约样本：TypeScript 与 Rust 两侧共同对着这一份文件校验状态的形状。
//
// 样本是未经类型收窄的 JSON，所以这里逐条用运行时守卫核对——这才是对形状的
// 真实检查，而不是让编译器相信一个断言。Rust 侧有对应的测试，把同一个文件
// 读进来再原样写出去。任何一边改了形状，都会有一侧失败。

import { describe, expect, test } from "bun:test";
import { isSnapshot } from "@/lib/guard";
import type { Notice, Snapshot } from "@/lib/types";
import samples from "../../contract/snapshots.json";

/** 样本必须全部通过守卫；通过之后才能当作 `Snapshot` 使用。 */
function checked(): Snapshot[] {
  const invalid = samples.filter((sample) => !isSnapshot(sample));
  expect(invalid).toEqual([]);
  return samples as Snapshot[];
}

describe("状态契约样本", () => {
  test("每一条样本都符合界面认得的形状", () => {
    expect(checked().length).toBeGreaterThan(0);
  });

  test("覆盖了会话的全部阶段", () => {
    const phases = new Set(checked().map((snapshot) => snapshot.session.phase));
    expect(phases).toEqual(new Set(["signedOut", "signingIn", "signedIn"]));
  });

  test("覆盖了开机启动的全部状态、连接的全部情况与提示类型", () => {
    const snapshots = checked();
    expect(
      new Set(snapshots.map((snapshot) => snapshot.launchAtLogin)),
    ).toEqual(new Set(["enabled", "disabled", "unsupported", "unknown"]));
    expect(
      new Set(snapshots.map((snapshot) => snapshot.service.reachable)),
    ).toEqual(new Set([true, false, null]));
    const notices = snapshots
      .map((snapshot) => snapshot.notice)
      .filter((notice): notice is Notice => notice !== null);
    expect(new Set(notices.map((notice) => notice.kind))).toEqual(
      new Set(["info", "error"]),
    );
  });

  test("样本里不出现任何凭据或本机路径", () => {
    const text = JSON.stringify(checked());
    for (const forbidden of [
      "token",
      "Bearer",
      "password",
      "/Users/",
      "cookie",
    ]) {
      expect(text.includes(forbidden)).toBe(false);
    }
  });
});
