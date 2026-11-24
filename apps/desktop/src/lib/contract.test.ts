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

  test("覆盖了传输的每个阶段、每种失败原因、两个方向、有没有草稿", () => {
    const snapshots = checked();
    const transfers = snapshots.flatMap((snapshot) => snapshot.transfers);
    expect(new Set(transfers.map((t) => t.phase))).toEqual(
      new Set([
        "queued",
        "inProgress",
        "waiting",
        "complete",
        "failed",
        "declined",
        "cancelled",
        "expired",
      ]),
    );
    expect(new Set(transfers.map((t) => t.direction))).toEqual(
      new Set(["in", "out"]),
    );
    expect(new Set(transfers.map((t) => t.failure))).toEqual(
      new Set([
        null,
        "diskFull",
        "writeFailed",
        "verificationFailed",
        "deviceOffline",
        "tooLarge",
        "fileUnreadable",
        "quotaExceeded",
        "vanished",
      ]),
    );
    expect(new Set(transfers.map((t) => t.target))).toEqual(
      new Set(["device", "drive"]),
    );
    expect(new Set(snapshots.map((s) => s.drive === null))).toEqual(
      new Set([true, false]),
    );
    expect(new Set(snapshots.map((s) => s.sendMenu))).toEqual(
      new Set(["enabled", "disabled", "unsupported"]),
    );
    expect(new Set(transfers.map((t) => t.retryable))).toEqual(
      new Set([true, false]),
    );
    expect(new Set(snapshots.map((s) => s.draft === null))).toEqual(
      new Set([true, false]),
    );
    expect(
      new Set(
        snapshots.map((snapshot) => snapshot.receive.customFolder === null),
      ),
    ).toEqual(new Set([true, false]));
    expect(
      new Set(transfers.map((transfer) => transfer.savedAs === null)),
    ).toEqual(new Set([true, false]));

    // 只有失败的传输带失败原因；能重发的只有"文件没到服务端"的那几种失败。
    const neverReachedTheServer = [
      "deviceOffline",
      "tooLarge",
      "fileUnreadable",
      "rejected",
      // 没存进云盘的，云盘里自然没有它。
      "quotaExceeded",
    ];
    for (const transfer of transfers) {
      expect(transfer.failure !== null).toBe(transfer.phase === "failed");
      expect(transfer.retryable).toBe(
        transfer.phase === "failed" &&
          transfer.direction === "out" &&
          neverReachedTheServer.includes(transfer.failure ?? ""),
      );
      // 等对方接收、被拒收，只出现在发出去的传输上。
      if (transfer.phase === "waiting" || transfer.phase === "declined") {
        expect(transfer.direction).toBe("out");
        expect(transfer.target).toBe("device");
      }
      // 另一头是云盘的，名字就叫"云盘"。
      expect(transfer.peer === "云盘").toBe(transfer.target === "drive");
      // "云盘里已经没有这个文件了"只会出现在取回的时候。
      if (transfer.failure === "vanished") {
        expect(transfer.direction).toBe("in");
        expect(transfer.target).toBe("drive");
      }
      // 收好了、发出去了进度就是 1；传输中在 0 与 1 之间；其余都是 0。
      const done =
        transfer.phase === "complete" || transfer.phase === "waiting";
      expect(transfer.progress === 1).toBe(done);
      expect(transfer.progress > 0 && transfer.progress < 1).toBe(
        transfer.phase === "inProgress",
      );
      // 只有收好的文件才有"实际存成了什么名字"。
      expect(transfer.savedAs !== null && transfer.phase !== "complete").toBe(
        false,
      );
      if (transfer.savedAs !== null) {
        expect(transfer.direction).toBe("in");
      }
    }
    // 草稿只跟着登录的会话出现；里面只有文件的名字与大小，没有位置。
    for (const draft of snapshots.flatMap((s) => (s.draft ? [s.draft] : []))) {
      expect(draft.files.length).toBeGreaterThan(0);
      expect(JSON.stringify(draft)).not.toContain("/");
    }
    // 云盘名单里只有名字、大小与时间：没有摘要，也没有位置。
    for (const drive of snapshots.flatMap((s) => (s.drive ? [s.drive] : []))) {
      expect(drive.limitBytes).toBeGreaterThan(0);
      expect(drive.usedBytes).toBeLessThanOrEqual(drive.limitBytes);
      expect(drive.items.length).toBeGreaterThan(0);
      const text = JSON.stringify(drive.items);
      expect(text).not.toContain("sha256");
      expect(text).not.toContain("/");
    }
    // 没登录就没有传输记录，也没有草稿和云盘名单。
    expect(
      snapshots
        .filter((snapshot) => snapshot.session.phase !== "signedIn")
        .every(
          (snapshot) =>
            snapshot.transfers.length === 0 &&
            snapshot.draft === null &&
            snapshot.drive === null,
        ),
    ).toBe(true);
  });

  test("覆盖了日程的每种重复规则、提醒的两种状态、有没有名单与定位", () => {
    const snapshots = checked();
    expect(new Set(snapshots.map((s) => s.planner === null))).toEqual(
      new Set([true, false]),
    );
    expect(new Set(snapshots.map((s) => s.plannerMuted))).toEqual(
      new Set([true, false]),
    );
    const planners = snapshots.flatMap((s) => (s.planner ? [s.planner] : []));
    expect(
      new Set(planners.flatMap((p) => p.schedules.map((s) => s.repeat))),
    ).toEqual(new Set(["none", "daily", "weekly", "monthly"]));
    expect(
      new Set(planners.flatMap((p) => p.reminders.map((r) => r.status))),
    ).toEqual(new Set(["scheduled", "fired"]));
    expect(new Set(planners.map((p) => p.schedules.length === 0))).toEqual(
      new Set([true, false]),
    );
    expect(new Set(planners.map((p) => p.focus))).toEqual(
      new Set([null, expect.any(String)]),
    );
    expect(new Set(planners.map((p) => p.compose))).toEqual(
      new Set([null, "reminder"]),
    );
    for (const planner of planners) {
      // 每场都按时间排，一场都没有的日程不占名单。
      for (const schedule of planner.schedules) {
        expect(schedule.occurrences.length).toBeGreaterThan(0);
        expect([...schedule.occurrences].sort()).toEqual(schedule.occurrences);
      }
      // 点开的那一条必须是名单里还没读的通知。
      if (planner.focus !== null) {
        expect(planner.notifications.some((n) => n.id === planner.focus)).toBe(
          true,
        );
      }
    }
    // 没登录就没有名单。
    expect(
      snapshots
        .filter((snapshot) => snapshot.session.phase !== "signedIn")
        .every((snapshot) => snapshot.planner === null),
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
