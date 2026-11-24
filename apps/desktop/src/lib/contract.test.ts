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

/** 会话里这台设备自己的标识；没登录时为 null。 */
function thisDevice(session: Snapshot["session"]): string | null {
  return session.phase === "signedIn" ? session.device.id : null;
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
    expect(
      new Set(snapshots.map((snapshot) => snapshot.service.stream)),
    ).toEqual(new Set(["off", "connecting", "connected", "reconnecting"]));
    // 没登录就没有设备列表；登录了列表里有自己，也有别的设备。
    expect(
      snapshots
        .filter((snapshot) => snapshot.session.phase !== "signedIn")
        .every((snapshot) => snapshot.devices.length === 0),
    ).toBe(true);
    // 逐条判断阶段，类型才知道会话里一定有设备。
    const signedIn = snapshots.flatMap((snapshot) =>
      snapshot.session.phase === "signedIn" ? [snapshot] : [],
    );
    expect(signedIn.length).toBeGreaterThan(0);
    expect(
      signedIn.every((snapshot) =>
        snapshot.devices.some(
          (device) => device.id === thisDevice(snapshot.session),
        ),
      ),
    ).toBe(true);
    expect(
      new Set(
        snapshots.flatMap((snapshot) => snapshot.devices).map((d) => d.online),
      ),
    ).toEqual(new Set([true, false]));
    const notices = snapshots
      .map((snapshot) => snapshot.notice)
      .filter((notice): notice is Notice => notice !== null);
    expect(new Set(notices.map((notice) => notice.kind))).toEqual(
      new Set(["info", "error"]),
    );
  });

  test("覆盖了传输的每个阶段、几种失败原因与两种接收位置", () => {
    const snapshots = checked();
    const transfers = snapshots.flatMap((snapshot) => snapshot.transfers);
    expect(new Set(transfers.map((transfer) => transfer.phase))).toEqual(
      new Set([
        "queued",
        "inProgress",
        "complete",
        "failed",
        "cancelled",
        "expired",
      ]),
    );
    expect(new Set(transfers.map((transfer) => transfer.failure))).toEqual(
      new Set([null, "diskFull", "writeFailed", "verificationFailed"]),
    );
    expect(
      new Set(
        snapshots.map((snapshot) => snapshot.receive.customFolder === null),
      ),
    ).toEqual(new Set([true, false]));
    expect(
      new Set(transfers.map((transfer) => transfer.savedAs === null)),
    ).toEqual(new Set([true, false]));

    // 只有失败的传输带失败原因；进度只在传输中才有意义，收好了就是 1。
    for (const transfer of transfers) {
      expect(transfer.failure !== null).toBe(transfer.phase === "failed");
      // 收好了进度就是 1，传输中在 0 与 1 之间，其余都是 0。
      expect(transfer.progress === 1).toBe(transfer.phase === "complete");
      expect(transfer.progress > 0 && transfer.progress < 1).toBe(
        transfer.phase === "inProgress",
      );
      // 只有收好的文件才有"实际存成了什么名字"。
      expect(transfer.savedAs !== null && transfer.phase !== "complete").toBe(
        false,
      );
    }
    // 没登录就没有传输记录。
    expect(
      snapshots
        .filter((snapshot) => snapshot.session.phase !== "signedIn")
        .every((snapshot) => snapshot.transfers.length === 0),
    ).toBe(true);
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
