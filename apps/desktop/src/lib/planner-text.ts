// 日程卡片用的文字换算：UTC 时间与本地时间之间，以及几种标签。
//
// 服务端给的时间都是 UTC；共享日期时间选择器和显示用的都是这台电脑的本地时间。
// 这里都是纯函数，不碰界面。

import { text, textLocale } from "@/lib/text-i18n";
import type {
  PlannerView,
  ReminderView,
  Repeat,
  ScheduleView,
} from "@/lib/types";

/** 这一天在本机时区下从哪一刻开始。 */
function startOfDay(date: Date): number {
  return new Date(
    date.getFullYear(),
    date.getMonth(),
    date.getDate(),
  ).getTime();
}

const pad = (value: number) => String(value).padStart(2, "0");

/** 给日期时间选择器用的本地时间值：`YYYY-MM-DDTHH:mm`。 */
export function toLocalInput(iso: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}T${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

/**
 * 输入框里的本地时间换回 UTC。按这台电脑的本地时间理解它。
 * 填得不完整（只有日期没有时刻、或者根本是空的）时为 null。
 */
export function fromLocalInput(value: string): string | null {
  // 选择器交出的值固定是 `YYYY-MM-DDTHH:mm`（秒可以不带）。
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?$/.exec(
    value,
  );
  if (!match) return null;
  const [year, month, day, hour, minute] = match.slice(1).map(Number);
  // 手动输入不再有原生时间框兜底，避免小时或分钟溢出被 Date 悄悄顺延。
  if (hour > 23 || minute > 59 || Number(match[6] ?? 0) > 59) return null;
  const at = new Date(year, month - 1, day, hour, minute);
  // 2 月 30 日这样的日期会被 Date 顺到下个月去：填错了就是填错了。
  const honest =
    at.getFullYear() === year &&
    at.getMonth() === month - 1 &&
    at.getDate() === day;
  return honest ? at.toISOString() : null;
}

/** `HH:mm`。 */
export function clock(iso: string): string {
  const at = new Date(iso);
  return `${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

/** 相对"现在"是哪一天：昨天、今天、明天、后天，本周内说周几，再远说月日。 */
export function dayLabel(iso: string, now: number): string {
  const at = new Date(iso);
  const days = Math.round(
    (startOfDay(at) - startOfDay(new Date(now))) / 86_400_000,
  );
  if (days === 0) return text("planner.day.today");
  if (days === -1) return text("planner.day.yesterday");
  if (days === 1) return text("planner.day.tomorrow");
  if (days === 2) return text("planner.day.dayAfterTomorrow");
  if (days > 2 && days < 7) {
    return new Intl.DateTimeFormat(textLocale(), { weekday: "short" }).format(
      at,
    );
  }
  return monthDay(iso, now, false);
}

export function repeatLabel(repeat: Repeat): string {
  return text(`planner.repeat.${repeat}`);
}

/** 可以选的提前量：到点，或者提前这么多分钟。 */
const LEAD_MINUTES = [null, 10, 30, 60, 1440] as const;

/** 组件每次渲染时读取，语言切换后不会继续使用导入时的文字。 */
export function leadOptions(): ReadonlyArray<readonly [number | null, string]> {
  return LEAD_MINUTES.map((minutes) => [
    minutes,
    minutes === null ? text("planner.lead.atTime") : leadLabel(minutes),
  ]);
}

/** @deprecated 表单请在渲染时调用 leadOptions()，此常量只保留导入兼容。 */
export const LEADS = leadOptions();

export function leadLabel(minutes: number): string {
  if (minutes === 60) return text("planner.lead.hours", { count: 1 });
  if (minutes === 1440) return text("planner.lead.days", { count: 1 });
  return text("planner.lead.minutes", { count: minutes });
}

/** 一条通知是多久之前生成的。 */
export function ago(iso: string, now: number): string {
  const raised = new Date(iso).getTime();
  const minutes = Math.floor((now - raised) / 60_000);
  if (minutes < 1) return text("planner.ago.justNow");
  if (minutes < 60) return text("planner.ago.minutes", { count: minutes });
  const hours = Math.floor(minutes / 60);
  if (
    hours < 24 &&
    startOfDay(new Date(raised)) === startOfDay(new Date(now))
  ) {
    return text("planner.ago.hours", { count: hours });
  }
  return `${dayLabel(iso, now)} ${clock(iso)}`;
}

/** `M月D日`；不是 `now` 那一年的，前面带上年份。 */
function monthDay(iso: string, now: number, includeYear = true): string {
  const at = new Date(iso);
  const locale = textLocale();
  return new Intl.DateTimeFormat(locale, {
    // 语言只决定格式；不设置 timeZone，继续按本机的年月日显示。
    month: locale === "en" ? "short" : "long",
    day: "numeric",
    ...(includeYear && at.getFullYear() !== new Date(now).getFullYear()
      ? { year: "numeric" as const }
      : {}),
  }).format(at);
}

/** 安排里的一行：一条日程按它最近的一场排，一条提醒按到点时间排。 */
export type Entry =
  | { key: string; at: number; schedule: ScheduleView }
  | { key: string; at: number; reminder: ReminderView };

/**
 * 七天之内的安排排成一份，早的在前。
 *
 * 日程按"还没到的最近一场"排：天天有的周会，今天这一场过了就排到明天那一场上。
 * 不重复、而且今天已经过去的那一条没有下一场，还按它的开始时间排，过完今天就不在名单里了。
 */
export function agenda(planner: PlannerView, now: number): Entry[] {
  const entries: Entry[] = [
    ...planner.schedules.map((schedule): Entry => {
      const at = placing(schedule, now);
      return {
        key: `schedule:${schedule.id}`,
        at: at === null ? Number.POSITIVE_INFINITY : new Date(at).getTime(),
        schedule,
      };
    }),
    ...planner.reminders.map((reminder): Entry => {
      return {
        key: `reminder:${reminder.id}`,
        at: new Date(reminder.dueAt).getTime(),
        reminder,
      };
    }),
  ];
  return entries.sort((a, b) => a.at - b.at);
}

/** 还没到的最近一场在名单里的位置；今天这一场也过去了就是 -1。 */
function upcoming(schedule: ScheduleView, now: number): number {
  return schedule.occurrences.findIndex((at) => new Date(at).getTime() >= now);
}

/** 一条日程在安排里按哪一场排。名单里不会有七天之内一场都没有的日程，但还是防一手。 */
function placing(schedule: ScheduleView, now: number): string | null {
  const next = upcoming(schedule, now);
  return schedule.occurrences[next === -1 ? 0 : next] ?? null;
}

/** 一条日程什么时候开始、怎么重复、提前多久提醒。 */
export function scheduleLine(schedule: ScheduleView, now: number): string {
  const parts: string[] = [];
  const at = placing(schedule, now);
  if (at !== null) {
    const time = schedule.endAt
      ? text("planner.timeSpan", {
          start: clock(at),
          end: clock(schedule.endAt),
        })
      : clock(at);
    parts.push(text("planner.at", { day: dayLabel(at, now), time }));
  }
  if (schedule.repeat !== "none") {
    const repeat = repeatLabel(schedule.repeat);
    parts.push(
      schedule.repeatUntil
        ? text("planner.repeatUntil", {
            repeat,
            date: monthDay(schedule.repeatUntil, now),
          })
        : repeat,
    );
  }
  if (schedule.remindBefore !== null) {
    parts.push(leadLabel(schedule.remindBefore));
  }
  return parts.join(" · ");
}

/**
 * 摆出来的那一场（还没到的最近一场）之后还有哪几场；没有了返回 null。
 * 只说最近的三场，再多的报个数。
 */
export function laterLabel(schedule: ScheduleView, now: number): string | null {
  const next = upcoming(schedule, now);
  const rest = next === -1 ? [] : schedule.occurrences.slice(next + 1);
  if (rest.length === 0) return null;
  const shown = rest
    .slice(0, 3)
    .map((iso) => `${monthDay(iso, now)} ${clock(iso)}`);
  const more =
    rest.length > 3
      ? text("planner.later.totalSuffix", { count: rest.length })
      : "";
  return text("planner.later.label", {
    dates: shown.join(text("planner.later.separator")),
    more,
  });
}

/** 一条提醒什么时候到点；已经到点的说"已到点"。 */
export function reminderLine(reminder: ReminderView, now: number): string {
  const when = `${dayLabel(reminder.dueAt, now)} ${clock(reminder.dueAt)}`;
  return reminder.status === "fired" ? text("planner.fired", { when }) : when;
}

/** 表单里填的东西。时间都是输入框里的本地时间；没填的就是空串。 */
export interface PlannerDraft {
  title: string;
  /** 日程的开始时间，或者提醒的到点时间。 */
  at: string;
  /** 日程的结束时间；提醒没有这一项。 */
  end: string;
}

/**
 * 表单里哪一项还没填好；都可以了返回 null。
 *
 * 规则本身在 Rust 那边（与服务端同源），发出去之前还会再校验一次；
 * 这里只是把最常见的三种填错早一步说清楚，省得等一个来回。
 */
export function problemWith(draft: PlannerDraft): string | null {
  if (draft.title.trim().length === 0)
    return text("planner.form.titleRequired");
  const start = fromLocalInput(draft.at);
  if (start === null) return text("planner.form.timeIncomplete");
  const end = draft.end ? fromLocalInput(draft.end) : null;
  if (draft.end && end === null) return text("planner.form.endIncomplete");
  if (end !== null && end <= start) return text("planner.form.endBeforeStart");
  return null;
}
