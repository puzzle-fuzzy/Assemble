import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { BridgeError } from "@/lib/bridge";
import { buildInfoLabels, formatBuildInfo } from "@/lib/build-info";
import { messageFor } from "@/lib/messages";
import {
  ago,
  clock,
  dayLabel,
  fromLocalInput,
  laterLabel,
  leadLabel,
  leadOptions,
  problemWith,
  reminderLine,
  repeatLabel,
  scheduleLine,
  toLocalInput,
} from "@/lib/planner-text";
import { text } from "@/lib/text-i18n";
import {
  formatSize,
  formatSpace,
  isSavedHere,
  transferStatus,
  verdict,
} from "@/lib/transfer-text";
import type { ScheduleView, TransferFailure, TransferView } from "@/lib/types";
import en from "@/locales/text.en.json";
import zhCN from "@/locales/text.zh-CN.json";

beforeEach(() => {
  setLanguage("zh-CN");
});
afterEach(() => {
  setLanguage("zh-CN");
});

const local = (day: number, hour = 0, minute = 0) =>
  new Date(2026, 10, day, hour, minute);
const now = local(25, 12).getTime();

function schedule(overrides: Partial<ScheduleView> = {}): ScheduleView {
  const at = local(26, 8).toISOString();
  return {
    id: "schedule-1",
    version: 1,
    createdBy: "用户的 MacBook",
    title: "火车 Train",
    location: { label: "Central 站", address: "用户地址" },
    startAt: at,
    endAt: local(26, 9).toISOString(),
    repeat: "none",
    repeatUntil: null,
    remindBefore: 30,
    notes: "user@example.test",
    occurrences: [at],
    ...overrides,
  };
}

function transfer(overrides: Partial<TransferView> = {}): TransferView {
  return {
    id: "transfer-1",
    direction: "out",
    peer: "用户的 Windows",
    name: "报告 Report.pdf",
    size: 1024,
    phase: "inProgress",
    progress: 0.456,
    savedAs: null,
    failure: null,
    retryable: false,
    target: "device",
    ...overrides,
  };
}

function leaves(resource: object, prefix = ""): Array<[string, string]> {
  return Object.entries(resource).flatMap(([key, value]) => {
    const path = prefix ? `${prefix}.${key}` : key;
    return typeof value === "string"
      ? [[path, value] as [string, string]]
      : leaves(value, path);
  });
}

describe("desktopText 语言与变量", () => {
  test("两份字典的系统文案和变量匹配，英文不依赖中文回退", () => {
    const normalized = (resource: object) =>
      new Map(
        leaves(resource).map(([key, value]) => [
          key.replace(/_(one|other)$/, ""),
          [...value.matchAll(/{{(\w+)}}/g)].map((match) => match[1]).sort(),
        ]),
      );
    expect(normalized(en)).toEqual(normalized(zhCN));
  });

  test("重复调用立即采用当前语言，插值原样保留调用者的内容", () => {
    expect(text("planner.day.today")).toBe("今天");
    expect(text("planner.at", { day: "自定义 Day", time: "08:05" })).toBe(
      "自定义 Day 08:05",
    );
    setLanguage("en");
    expect(text("planner.day.today")).toBe("Today");
    expect(text("planner.at", { day: "自定义 Day", time: "08:05" })).toBe(
      "自定义 Day 08:05",
    );
    expect(text("planner.lead.minutes", { count: 1 })).toBe("1 minute before");
    expect(text("planner.lead.minutes", { count: 2 })).toBe("2 minutes before");
    setLanguage("zh-CN");
    expect(text("planner.day.today")).toBe("今天");
    expect(text("planner.lead.minutes", { count: 2 })).toBe("提前 2 分钟");
  });

  test("构建类型和官网标签跟随语言，版本号和提交号保持原值", () => {
    const info = {
      channel: "stable" as const,
      version: "0.1.0",
      commit: "abcdef1",
      website: "https://as.yxswy.com",
    };
    expect(formatBuildInfo(info)).toBe("正式版本 - 0.1.0.abcdef1");
    expect(buildInfoLabels()).toEqual({ version: "当前版本", website: "官网" });
    setLanguage("en");
    expect(formatBuildInfo(info)).toBe("Stable release - 0.1.0.abcdef1");
    expect(formatBuildInfo({ ...info, channel: "preview" })).toBe(
      "Preview release - 0.1.0.abcdef1",
    );
    expect(formatBuildInfo({ ...info, channel: "local" })).toBe(
      "Local build - 0.1.0.abcdef1",
    );
    expect(buildInfoLabels()).toEqual({
      version: "Current version",
      website: "Website",
    });
    expect(info.website).toBe("https://as.yxswy.com");
  });
});

describe("日程格式与本地时区", () => {
  test("英文日期、重复、提前量、相对时间和复数", () => {
    setLanguage("en");
    expect(dayLabel(local(24, 9).toISOString(), now)).toBe("Yesterday");
    expect(dayLabel(local(25, 9).toISOString(), now)).toBe("Today");
    expect(dayLabel(local(26, 9).toISOString(), now)).toBe("Tomorrow");
    expect(dayLabel(local(27, 9).toISOString(), now)).toBe(
      "Day after tomorrow",
    );
    expect(dayLabel(local(29, 9).toISOString(), now)).toBe("Sun");
    expect(dayLabel(local(32, 9).toISOString(), now)).toBe("Dec 2");
    expect(
      ["none", "daily", "weekly", "monthly"].map((repeat) =>
        repeatLabel(repeat as ScheduleView["repeat"]),
      ),
    ).toEqual(["Does not repeat", "Daily", "Weekly", "Monthly"]);
    expect(leadOptions()).toEqual([
      [null, "At the scheduled time"],
      [10, "10 minutes before"],
      [30, "30 minutes before"],
      [60, "1 hour before"],
      [1440, "1 day before"],
    ]);
    expect(leadLabel(1)).toBe("1 minute before");
    expect(leadLabel(45)).toBe("45 minutes before");
    expect(ago(local(25, 11, 59).toISOString(), now)).toBe("1 minute ago");
    expect(ago(local(25, 11, 58).toISOString(), now)).toBe("2 minutes ago");
    expect(ago(local(25, 11).toISOString(), now)).toBe("1 hour ago");
    expect(ago(local(25, 9).toISOString(), now)).toBe("3 hours ago");
    expect(ago(local(25, 11, 59).toISOString(), now - 30_000)).toBe("Just now");
    expect(ago(local(24, 22).toISOString(), now)).toBe("Yesterday 22:00");
  });

  test("语言改变格式而不改变本地时间，中文原有格式保留", () => {
    const iso = local(25, 0, 5).toISOString();
    const before = {
      clock: clock(iso),
      input: toLocalInput(iso),
      utc: fromLocalInput("2026-11-25T00:05"),
    };
    expect(dayLabel(local(32).toISOString(), now)).toBe("12月2日");
    setLanguage("en");
    expect({
      clock: clock(iso),
      input: toLocalInput(iso),
      utc: fromLocalInput("2026-11-25T00:05"),
    }).toEqual(before);
    expect(clock(iso)).toBe("00:05");
    expect(dayLabel(iso, now)).toBe("Today");
    setLanguage("zh-CN");
    expect(leadOptions()[3]).toEqual([60, "提前 1 小时"]);
  });

  test("完整日程和后续场次不修改用户内容", () => {
    const item = schedule({
      repeat: "weekly",
      repeatUntil: new Date(2027, 0, 1).toISOString(),
      occurrences: Array.from({ length: 7 }, (_, day) =>
        local(26 + day, 8).toISOString(),
      ),
    });
    const original = structuredClone(item);
    setLanguage("en");
    expect(scheduleLine(item, now)).toBe(
      "Tomorrow 08:00 to 09:00 · Weekly, until Jan 1, 2027 · 30 minutes before",
    );
    expect(laterLabel(item, now)).toBe(
      "Later: Nov 27 08:00, Nov 28 08:00, Nov 29 08:00 (6 occurrences total)",
    );
    expect(
      reminderLine(
        {
          id: "r1",
          version: 1,
          createdBy: "用户的 MacBook",
          title: "提醒",
          dueAt: local(25, 10).toISOString(),
          status: "fired",
          notes: null,
        },
        now,
      ),
    ).toBe("Due · Today 10:00");
    expect(item).toEqual(original);
  });

  test("英文表单错误与中文含义对应，不把表单内容当作翻译键", () => {
    const fine = { title: "火车 Train", at: "2026-11-26T09:00", end: "" };
    setLanguage("en");
    expect(problemWith(fine)).toBeNull();
    expect(problemWith({ ...fine, title: " " })).toBe("Enter a title first.");
    expect(problemWith({ ...fine, at: "" })).toBe(
      "Enter a complete date and time.",
    );
    expect(problemWith({ ...fine, end: "2026-11-26" })).toBe(
      "Enter a complete end time, or leave it empty.",
    );
    expect(problemWith({ ...fine, end: "2026-11-26T09:00" })).toBe(
      "The end time must be after the start time.",
    );
    expect(fine.title).toBe("火车 Train");
  });
});

describe("传输与安全错误文案", () => {
  test("重启后中断的记录有明确状态，保留用户文字", () => {
    const item = transfer({ phase: "failed", failure: "interrupted" });
    expect(transferStatus(item)).toBe("已中断");
    setLanguage("en");
    expect(transferStatus(item)).toBe("Interrupted");
    expect(transferStatus({ ...item, direction: "in" })).toBe("Interrupted");
    expect(item.name).toBe("报告 Report.pdf");
  });
  test("发送、接收、上传和取回的进度及完成状态均跟随语言", () => {
    const item = transfer();
    const original = structuredClone(item);
    expect(transferStatus(item)).toBe("正在发送 45%");
    expect(
      transferStatus(transfer({ direction: "in", phase: "complete" })),
    ).toBe("已保存");
    setLanguage("en");
    for (const [direction, target, running, done] of [
      ["out", "device", "Sending 45%", "Received by the other device"],
      ["in", "device", "Receiving 45%", "Saved"],
      ["out", "drive", "Uploading to cloud drive 45%", "Saved to cloud drive"],
      ["in", "drive", "Downloading from cloud drive 45%", "Downloaded"],
    ] as const) {
      expect(transferStatus(transfer({ direction, target }))).toBe(running);
      expect(
        transferStatus(transfer({ direction, target, phase: "complete" })),
      ).toBe(done);
      expect(
        transferStatus(transfer({ direction, target, phase: "queued" })),
      ).toMatch(/^Waiting to/);
    }
    expect(item).toEqual(original);
    expect(item.name).toBe("报告 Report.pdf");
    expect(item.peer).toBe("用户的 Windows");
  });

  test("失败原因区分收发两端，未知或不适用的原因使用通用说法", () => {
    expect(
      transferStatus(transfer({ phase: "failed", failure: "diskFull" })),
    ).toBe("发送失败：对方设备的磁盘已满。");
    setLanguage("en");
    expect(
      transferStatus(transfer({ phase: "failed", failure: "diskFull" })),
    ).toBe("Send failed: The other device's disk is full.");
    expect(
      transferStatus(
        transfer({ direction: "in", phase: "failed", failure: "diskFull" }),
      ),
    ).toBe("Receive failed: This computer's disk is full.");
    expect(
      transferStatus(
        transfer({
          direction: "in",
          target: "drive",
          phase: "failed",
          failure: "vanished",
        }),
      ),
    ).toBe("Download failed: This file is no longer in the cloud drive.");
    expect(
      transferStatus(
        transfer({ direction: "in", phase: "failed", failure: "tooLarge" }),
      ),
    ).toBe("Receive failed: Unknown reason.");
    expect(transferStatus(transfer({ phase: "failed" }))).toBe(
      "Send failed: Unknown reason.",
    );
    for (const failure of Object.keys(
      zhCN.transfer.failures,
    ) as TransferFailure[]) {
      for (const direction of ["in", "out"] as const) {
        const label = transferStatus(
          transfer({ direction, failure, phase: "failed" }),
        );
        expect(label).not.toMatch(/[\u3400-\u9fff]|transfer\./);
      }
    }
  });

  test("等待、拒收、撤回和过期状态翻译，传输规则保持原值", () => {
    setLanguage("en");
    expect(transferStatus(transfer({ phase: "waiting" }))).toBe(
      "Sent, waiting for the other device to receive",
    );
    expect(transferStatus(transfer({ phase: "declined" }))).toBe(
      "The other device declined the file.",
    );
    expect(transferStatus(transfer({ phase: "cancelled" }))).toBe("Cancelled.");
    expect(
      transferStatus(transfer({ direction: "in", phase: "cancelled" })),
    ).toBe("The sender withdrew the file. Nothing was received.");
    expect(transferStatus(transfer({ phase: "expired" }))).toBe(
      "The other device did not receive the file before it expired.",
    );
    expect(
      transferStatus(transfer({ direction: "in", phase: "expired" })),
    ).toBe("The transfer expired. Nothing was received.");
    expect(isSavedHere(transfer({ direction: "in", phase: "complete" }))).toBe(
      true,
    );
    expect(verdict([transfer()])).toBe("sending");
    expect(verdict([transfer({ phase: "failed" })])).toBe("failed");
    expect(verdict([transfer({ phase: "complete" })])).toBe("delivered");
  });

  test("两种语言都保持容量单位与原有精度", () => {
    for (const language of ["zh-CN", "en"] as const) {
      setLanguage(language);
      expect([0, 1000, 1024, 10_240, 1_048_576].map(formatSize)).toEqual([
        "0 B",
        "1000 B",
        "1.0 KB",
        "10 KB",
        "1.0 MB",
      ]);
      expect(formatSpace(1_073_741_824)).toBe("1.0 GB");
    }
  });

  test("错误码保持固定，所有英文错误均不泄漏 Rust 原始诊断", () => {
    const detail = "private-system-detail 10.0.0.1 /private/path";
    expect(messageFor(new BridgeError("UNREACHABLE", detail))).toBe(
      "暂时连不上服务，请检查网络后重试。",
    );
    setLanguage("en");
    expect(messageFor(new BridgeError("UNREACHABLE", detail))).toBe(
      "The service is unavailable. Check your network and try again.",
    );
    for (const code of [
      ...Object.keys(zhCN.errors),
      "UNKNOWN",
      "constructor",
      "__proto__",
    ]) {
      const error = new BridgeError(code, detail);
      const label = messageFor(error);
      expect(error.code).toBe(code);
      expect(label).not.toContain(detail);
      expect(label).not.toMatch(/[\u3400-\u9fff]|errors\./);
    }
    expect(messageFor(new BridgeError("UNKNOWN", detail))).toBe(
      "The action could not be completed. Try again later.",
    );
  });
});
