import { describe, expect, test } from "bun:test";

import {
  agenda,
  ago,
  clock,
  dayLabel,
  type Entry,
  fromLocalInput,
  laterLabel,
  leadLabel,
  problemWith,
  reminderLine,
  repeatLabel,
  scheduleLine,
  toLocalInput,
} from "@/lib/planner-text";
import type { PlannerView, ScheduleView } from "@/lib/types";

/** 这台电脑的本地时间；期望值用同一套 Date 接口算出来，不写死时区。 */
function local(year: number, month: number, day: number, hour = 0, minute = 0) {
  return new Date(year, month - 1, day, hour, minute);
}

const at = (date: Date) => date.toISOString();

describe("planner-text", () => {
  test("UTC 与输入框里的本地时间互逆", () => {
    const iso = local(2026, 11, 25, 9, 5).toISOString();
    expect(toLocalInput(iso)).toBe("2026-11-25T09:05");
    expect(fromLocalInput("2026-11-25T09:05")).toBe(iso);
    // 带秒的也认。
    expect(fromLocalInput("2026-11-25T09:05:00")).toBe(iso);
  });

  test("填得不完整或者填错了的日期不悄悄变成别的时刻", () => {
    for (const broken of ["", "2026-11-25", "2026-11-25T09", "明天", "abc"]) {
      expect(fromLocalInput(broken)).toBeNull();
    }
    // 2 月没有 30 日。
    expect(fromLocalInput("2026-02-30T09:00")).toBeNull();
    expect(toLocalInput("不是时间")).toBe("");
  });

  test("哪一天、几点、怎么重复", () => {
    const now = local(2026, 11, 25, 12).getTime();
    expect(dayLabel(local(2026, 11, 25, 9).toISOString(), now)).toBe("今天");
    expect(dayLabel(local(2026, 11, 26, 9).toISOString(), now)).toBe("明天");
    expect(dayLabel(local(2026, 11, 27, 9).toISOString(), now)).toBe("后天");
    // 2026-11-25 是周三。七天之内说周几：周日、下周一、下周二；第七天起说月日。
    expect(dayLabel(local(2026, 11, 29, 9).toISOString(), now)).toBe("周日");
    expect(dayLabel(local(2026, 11, 30, 9).toISOString(), now)).toBe("周一");
    expect(dayLabel(local(2026, 12, 1, 9).toISOString(), now)).toBe("周二");
    expect(dayLabel(local(2026, 12, 2, 9).toISOString(), now)).toBe("12月2日");
    expect(dayLabel(local(2026, 11, 20, 9).toISOString(), now)).toBe(
      "11月20日",
    );
    expect(clock(local(2026, 11, 25, 8, 7).toISOString())).toBe("08:07");
    expect(repeatLabel("monthly")).toBe("每月");
    expect(leadLabel(30)).toBe("提前 30 分钟");
    expect(leadLabel(45)).toBe("提前 45 分钟");
  });

  test("一条通知是多久之前来的", () => {
    const now = local(2026, 11, 25, 12).getTime();
    expect(ago(new Date(2026, 10, 25, 11, 59, 30).toISOString(), now)).toBe(
      "刚刚",
    );
    expect(ago(local(2026, 11, 25, 11, 40).toISOString(), now)).toBe(
      "20 分钟前",
    );
    expect(ago(local(2026, 11, 25, 9).toISOString(), now)).toBe("3 小时前");
    expect(ago(local(2026, 11, 24, 22).toISOString(), now)).toBe("昨天 22:00");
  });
});

/** 一条不重复的日程；每项测试自己改它。 */
function schedule(overrides: Partial<ScheduleView> = {}): ScheduleView {
  const start = at(local(2026, 11, 26, 8));
  return {
    id: "s1",
    version: 1,
    createdBy: "MacBook-Pro",
    title: "火车",
    location: null,
    startAt: start,
    endAt: null,
    repeat: "none",
    repeatUntil: null,
    remindBefore: null,
    notes: null,
    occurrences: [start],
    ...overrides,
  };
}

describe("接下来的安排", () => {
  // 2026-11-25 是周三。
  const now = local(2026, 11, 25, 12).getTime();
  const daily = schedule({
    id: "daily",
    title: "周会",
    startAt: at(local(2026, 11, 25, 9)),
    repeat: "daily",
    repeatUntil: at(local(2026, 12, 31)),
    occurrences: [
      at(local(2026, 11, 25, 9)),
      at(local(2026, 11, 26, 9)),
      at(local(2026, 11, 27, 9)),
    ],
  });
  const train = schedule({
    id: "train",
    endAt: at(local(2026, 11, 26, 9)),
    remindBefore: 30,
  });
  const weekly = schedule({
    id: "weekly",
    title: "周报",
    startAt: at(local(2026, 11, 30, 16)),
    endAt: at(local(2026, 11, 30, 17)),
    repeat: "weekly",
    occurrences: [at(local(2026, 11, 30, 16))],
  });
  const picked = {
    id: "picked",
    version: 2,
    status: "fired" as const,
    createdBy: "ZCode 的助手",
    title: "取快递",
    dueAt: at(local(2026, 11, 25, 10)),
    notes: null,
  };
  const call = {
    ...picked,
    id: "call",
    version: 1,
    status: "scheduled" as const,
    title: "给妈妈打电话",
    dueAt: at(local(2026, 11, 26, 20)),
  };

  function planner(): PlannerView {
    return {
      notifications: [],
      schedules: [weekly, daily, train],
      reminders: [call, picked],
      focus: null,
      compose: null,
    };
  }

  test("日程与提醒合成一份，按时间排；重复的按还没到的最近一场", () => {
    const keys = agenda(planner(), now).map((entry: Entry) => entry.key);
    expect(keys).toEqual([
      "reminder:picked",
      "schedule:train",
      // 周会今天 09:00 那一场已经过去了：排在明天那一场上。
      "schedule:daily",
      "reminder:call",
      "schedule:weekly",
    ]);
    // 不重复、今天已经过去的：没有下一场，还按它的开始时间排。
    const morning = at(local(2026, 11, 25, 9));
    const over = schedule({ startAt: morning, occurrences: [morning] });
    const single: PlannerView = { ...planner(), schedules: [over] };
    expect(agenda(single, now)[0].key).toBe("schedule:s1");
  });

  test("一行说清什么时候、怎么重复、提前多久", () => {
    expect(scheduleLine(train, now)).toBe("明天 08:00 到 09:00 · 提前 30 分钟");
    expect(scheduleLine(daily, now)).toBe("明天 09:00 · 每天，到 12月31日");
    expect(scheduleLine(weekly, now)).toBe("周一 16:00 到 17:00 · 每周");
  });

  test("重复的日程报出后面还有哪几场", () => {
    // 周会摆出来的是明天那一场：今天已经过去的不算在"之后"里。
    expect(laterLabel(daily, now)).toBe("之后：11月27日 09:00");
    expect(laterLabel(train, now)).toBeNull();
    const long = schedule({
      repeat: "daily",
      occurrences: Array.from({ length: 7 }, (_, day) =>
        at(local(2026, 11, 26 + day, 9)),
      ),
    });
    expect(laterLabel(long, now)).toBe(
      "之后：11月27日 09:00、11月28日 09:00、11月29日 09:00 等 6 场",
    );
  });

  test("到点的提醒说已经到点", () => {
    expect(reminderLine(picked, now)).toBe("已到点 · 今天 10:00");
    expect(reminderLine(call, now)).toBe("明天 20:00");
  });
});

describe("表单校验", () => {
  const fine = { title: "火车", at: "2026-11-26T09:00", end: "" };

  test("都填好了就没有问题；结束时间可以不填", () => {
    expect(problemWith(fine)).toBeNull();
    expect(problemWith({ ...fine, end: "2026-11-26T10:30" })).toBeNull();
  });

  test("最常见的三种填错各说各的", () => {
    expect(problemWith({ ...fine, title: "  " })).toBe("先写一个标题。");
    expect(problemWith({ ...fine, at: "" })).toBe(
      "时间要填到哪一天、几点几分。",
    );
    expect(problemWith({ ...fine, end: "2026-11-26" })).toBe(
      "结束时间没有填完整；不填也可以。",
    );
    expect(problemWith({ ...fine, end: "2026-11-26T09:00" })).toBe(
      "结束时间要晚于开始时间。",
    );
  });
});
