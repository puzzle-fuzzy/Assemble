// 日程卡片与"到点提醒"开关的界面测试：注入替身桥接，在真实 DOM 里验证看到的与触发的操作。
//
// 卡片上的文案都是相对"现在"说的，所以这里的"现在"钉在 2026-11-25（周三）中午十二点；
// 名单里的时间按本机时区写，期望值就不随时区变。

import {
  afterEach,
  beforeEach,
  describe,
  expect,
  setSystemTime,
  test,
} from "bun:test";
import { act } from "react";
import { BridgeError } from "@/lib/bridge";
import type { PlannerView } from "@/lib/types";
import {
  button,
  card,
  cleanup,
  click,
  field,
  fill,
  listUnder,
  rows,
  settle,
  switchNamed,
  text,
} from "@/test-support/dom";
import { DEVICE, FakeBridge, snapshot } from "@/test-support/fakes";
import { renderApp } from "@/test-support/render-app";

beforeEach(() => {
  document.body.replaceChildren();
  setSystemTime(new Date(2026, 10, 25, 12));
});

afterEach(() => {
  cleanup();
  setSystemTime();
});

/** 一个本机时区下的时刻，按服务端的写法（UTC）给出。 */
function utc(
  year: number,
  month: number,
  day: number,
  hour = 0,
  minute = 0,
): string {
  return new Date(year, month - 1, day, hour, minute).toISOString();
}

/** 离"现在"多久之前生成的一条通知。 */
function raised(secondsAgo: number): string {
  return new Date(
    new Date(2026, 10, 25, 12).getTime() - secondsAgo * 1000,
  ).toISOString();
}

/**
 * 一份名单：两条没读的通知，三条日程（每天的、一次性的、每周的），
 * 两条提醒（到点的、没到的）。有的记在本机上，有的记在助手那边。
 */
function listing(overrides: Partial<PlannerView> = {}): PlannerView {
  return {
    notifications: [
      {
        id: "note-picked",
        title: "取快递",
        body: "驿站楼下",
        scheduleId: null,
        reminderId: "reminder-1",
        raisedAt: utc(2026, 11, 25, 11, 40),
      },
      {
        id: "note-meeting",
        title: "周会",
        body: "三楼小会议室",
        scheduleId: "schedule-1",
        reminderId: null,
        raisedAt: raised(30),
      },
    ],
    schedules: [
      {
        id: "schedule-1",
        version: 1,
        createdBy: "ZCode 的助手",
        title: "周会",
        location: null,
        startAt: utc(2026, 11, 25, 9, 30),
        endAt: null,
        repeat: "daily",
        repeatUntil: null,
        remindBefore: null,
        notes: "三楼小会议室",
        occurrences: [
          utc(2026, 11, 25, 9, 30),
          utc(2026, 11, 26, 9, 30),
          utc(2026, 11, 27, 9, 30),
        ],
      },
      {
        id: "schedule-2",
        version: 3,
        createdBy: DEVICE.name,
        title: "火车",
        location: { label: "上海虹桥站", address: "申虹路 1500 号" },
        startAt: utc(2026, 11, 26, 9),
        endAt: utc(2026, 11, 26, 10),
        repeat: "none",
        repeatUntil: null,
        remindBefore: 30,
        notes: "带身份证",
        occurrences: [utc(2026, 11, 26, 9)],
      },
      {
        id: "schedule-3",
        version: 1,
        createdBy: "Codex 的助手",
        title: "周报",
        location: null,
        startAt: utc(2026, 11, 20, 16),
        endAt: utc(2026, 11, 20, 17),
        repeat: "weekly",
        repeatUntil: utc(2026, 12, 31),
        remindBefore: null,
        notes: null,
        occurrences: [utc(2026, 11, 30, 16)],
      },
    ],
    reminders: [
      {
        id: "reminder-1",
        version: 2,
        status: "fired",
        createdBy: "ZCode 的助手",
        title: "取快递",
        dueAt: utc(2026, 11, 25, 9),
        notes: "驿站楼下",
      },
      {
        id: "reminder-2",
        version: 1,
        status: "scheduled",
        createdBy: DEVICE.name,
        title: "给妈妈打电话",
        dueAt: utc(2026, 11, 26, 20),
        notes: null,
      },
    ],
    focus: null,
    compose: null,
    ...overrides,
  };
}

/** 通知那一行：同名的在名单里还有一行，通知的在前面。 */
async function notice(host: HTMLElement, title: string): Promise<HTMLElement> {
  return rows(host, title)[0];
}

/** 接下来的安排里的那一行。 */
async function planned(host: HTMLElement, title: string): Promise<HTMLElement> {
  const found = rows(host, title).at(-1);
  if (!found) throw new Error(`没有找到安排里的「${title}」`);
  return found;
}

/** 换掉 `scrollIntoView`，记下谁要求滚到眼前；测试用的 DOM 实现里原本没有它。 */
function trackScrolling() {
  const scrolled: Element[] = [];
  const prototype = HTMLElement.prototype;
  const own = Object.hasOwn(prototype, "scrollIntoView");
  const original = prototype.scrollIntoView;
  Object.defineProperty(prototype, "scrollIntoView", {
    configurable: true,
    value(this: Element) {
      scrolled.push(this);
    },
  });
  return {
    scrolled,
    restore() {
      if (own) Object.defineProperty(prototype, "scrollIntoView", original);
      else Reflect.deleteProperty(prototype, "scrollIntoView");
    },
  };
}

describe("日程", () => {
  test("没登录时没有这一块", async () => {
    const view = await renderApp(new FakeBridge(snapshot()), "日程与提醒");
    await settle();
    expect(
      view.host.querySelector('section:not([hidden]) [data-slot="card-title"]'),
    ).toBeNull();
  });

  test("列出到点的提醒，与七天之内按时间排好的安排", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", planner: listing() }),
    );
    const view = await renderApp(source, "日程与提醒");
    await settle();
    const page = card(view.host, "日程");

    // 通知说清楚是多久之前来的。
    expect(text(await notice(page, "取快递"))).toContain("驿站楼下");
    expect(text(await notice(page, "取快递"))).toContain("20 分钟前");
    expect(text(await notice(page, "周会"))).toContain("刚刚");

    // 今天 09:00 那条到点的提醒排最前；天天有的周会按明天那一场排。
    const order = [
      ...listUnder(page, "接下来的安排").querySelectorAll("h3"),
    ].map((heading) => heading.textContent);
    expect(order).toEqual(["取快递", "火车", "周会", "给妈妈打电话", "周报"]);
    expect(text(await planned(page, "取快递"))).toContain(
      "已到点 · 今天 09:00",
    );
    expect(text(await planned(page, "火车"))).toContain(
      "明天 09:00 到 10:00 · 提前 30 分钟",
    );
    expect(text(await planned(page, "火车"))).toContain("上海虹桥站");
    expect(text(await planned(page, "火车"))).toContain("带身份证");
    expect(text(await planned(page, "周会"))).toContain("明天 09:30 · 每天");
    expect(text(await planned(page, "周会"))).toContain("之后：11月27日 09:30");
    expect(text(await planned(page, "周报"))).toContain(
      "周一 16:00 到 17:00 · 每周，到 12月31日",
    );
    expect(text(await planned(page, "给妈妈打电话"))).toContain("明天 20:00");

    // 助手记下的标出来；本机自己记的不标。
    expect(text(await planned(page, "周会"))).toContain("ZCode 的助手");
    expect(text(await planned(page, "火车"))).not.toContain(DEVICE.name);

    // 每一行给自己的按钮：重复的日程没有"完成"，到点的提醒不能再改。
    expect(button(await planned(page, "火车"), "完成")).toBeDefined();
    expect(button(await planned(page, "周会"), "完成")).toBeUndefined();
    expect(button(await planned(page, "火车"), "改…")).toBeDefined();
    expect(button(await planned(page, "给妈妈打电话"), "改…")).toBeDefined();
    expect(button(await planned(page, "给妈妈打电话"), "办完了")).toBeDefined();
    expect(button(await planned(page, "取快递"), "看过了")).toBeDefined();
    expect(button(await planned(page, "取快递"), "改…")).toBeUndefined();
    expect(button(await planned(page, "给妈妈打电话"), "不办了")).toBeDefined();
    // 通知上：提醒的能顺手看完，都能"知道了"。
    expect(button(await notice(page, "取快递"), "看过了")).toBeDefined();
    expect(button(await notice(page, "周会"), "看过了")).toBeUndefined();
    expect(button(await notice(page, "周会"), "知道了")).toBeDefined();
    expect(source.calls).toEqual([]);
  });

  test("名单还没拿回来时向 Rust 要一次，不反复要", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", planner: null }),
    );
    const view = await renderApp(source, "日程与提醒");
    await settle();
    expect(source.calls).toEqual(["plannerRefresh"]);
    expect(
      card(view.host, "日程").querySelector(
        '[role="status"][aria-label="正在读取日程…"]',
      ),
    ).not.toBeNull();

    // 还是没拿到：不重复要。
    await act(async () => {
      source.push(snapshot({ phase: "signedIn", planner: null }));
    });
    await settle();
    expect(source.calls).toEqual(["plannerRefresh"]);
  });

  test("七天之内什么都没有时如实说", async () => {
    const empty = listing({
      notifications: [],
      schedules: [],
      reminders: [],
    });
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", planner: empty }),
    );
    const view = await renderApp(source, "日程与提醒");
    await settle();
    const page = card(view.host, "日程");
    expect(text(page)).toContain("七天之内没有安排。");
    expect(text(page)).not.toContain("到点的提醒");
    expect(text(page)).not.toContain("接下来的安排");
    expect(button(page, "全部已读")).toBeUndefined();
  });

  test("到点的提醒：知道了只标成已读，提醒可以顺手看完，多了能全部已读", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", planner: listing() }),
    );
    const view = await renderApp(source, "日程与提醒");
    await settle();
    const page = card(view.host, "日程");

    await click(button(await notice(page, "周会"), "知道了"));
    await click(button(await notice(page, "取快递"), "看过了"));
    // 只交出通知的标识；提醒带上看到的版本号。
    expect(source.calls).toEqual([
      "notificationsRead:note-meeting",
      "reminderDismiss:reminder-1:2",
    ]);
    await click(button(page, "全部已读"));
    expect(source.calls.at(-1)).toBe("notificationsReadAll");
  });

  test("只有一条通知时不摆“全部已读”", async () => {
    const one = listing({ notifications: [listing().notifications[0]] });
    const view = await renderApp(
      new FakeBridge(snapshot({ phase: "signedIn", planner: one })),
      "日程与提醒",
    );
    await settle();
    expect(button(card(view.host, "日程"), "全部已读")).toBeUndefined();
  });

  test("点开系统通知之后，滚到那一条并标出来", async () => {
    const scrolling = trackScrolling();
    try {
      const source = new FakeBridge(
        snapshot({
          phase: "signedIn",
          planner: listing({ focus: "note-meeting" }),
        }),
      );
      const view = await renderApp(source, "日程与提醒");
      await settle();
      const page = card(view.host, "日程");
      expect(scrolling.scrolled).toEqual([await notice(page, "周会")]);
      expect((await notice(page, "周会")).getAttribute("aria-current")).toBe(
        "true",
      );
      expect((await notice(page, "取快递")).hasAttribute("aria-current")).toBe(
        false,
      );
    } finally {
      scrolling.restore();
    }
  });

  test("日程：不重复的可以完成；取消要先确认", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", planner: listing() }),
    );
    const view = await renderApp(source, "日程与提醒");
    await settle();
    const page = card(view.host, "日程");

    await click(button(await planned(page, "火车"), "完成"));
    expect(source.calls).toEqual(["scheduleComplete:schedule-2:3"]);

    await click(button(await planned(page, "火车"), "取消"));
    expect(button(await planned(page, "火车"), "确认取消")).toBeDefined();
    expect(source.calls).toHaveLength(1);
    await click(button(await planned(page, "火车"), "算了"));
    expect(source.calls).toHaveLength(1);
    expect(button(await planned(page, "火车"), "取消")).toBeDefined();

    await click(button(await planned(page, "火车"), "取消"));
    await click(button(await planned(page, "火车"), "确认取消"));
    expect(source.calls.at(-1)).toBe("scheduleCancel:schedule-2:3");
  });

  test("提醒：办完了直接交回去；不办了要先确认", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", planner: listing() }),
    );
    const view = await renderApp(source, "日程与提醒");
    await settle();
    const page = card(view.host, "日程");

    await click(button(await planned(page, "给妈妈打电话"), "办完了"));
    expect(source.calls).toEqual(["reminderDismiss:reminder-2:1"]);

    await click(button(await planned(page, "取快递"), "不办了"));
    await click(button(await planned(page, "取快递"), "确认不办了"));
    expect(source.calls.at(-1)).toBe("reminderCancel:reminder-1:2");
  });

  test("新建提醒：最常见的填错在本地拦下，填好的换成 UTC 交给 Rust", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", planner: listing() }),
    );
    const view = await renderApp(source, "日程与提醒");
    await settle();
    let page = card(view.host, "日程");

    await click(button(page, "新建提醒…"));
    expect(text(page)).toContain("新建提醒");
    await click(button(page, "保存"));
    expect(text(page)).toContain("先写一个标题。");

    await fill(page, "标题", " 取快递 ");
    await click(button(page, "保存"));
    expect(text(page)).toContain("时间要填到哪一天、几点几分。");
    expect(source.calls).toEqual([]);

    await fill(page, "到点时间", "2026-11-26T09:05");
    await fill(page, "备注", " 驿站楼下 ");
    await click(button(page, "保存"));
    expect(source.calls).toEqual(["reminderCreate:取快递"]);
    expect(source.lastSubmitted).toEqual({
      title: "取快递",
      dueAt: utc(2026, 11, 26, 9, 5),
      notes: "驿站楼下",
    });
    // 存好了表单收起，名单又摆出来。
    page = card(view.host, "日程");
    expect(button(page, "新建提醒…")).toBeDefined();
    expect(button(page, "保存")).toBeUndefined();
  });

  test("改日程：带着看到的版本号整份替换，没改的地点原样留着", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", planner: listing() }),
    );
    const view = await renderApp(source, "日程与提醒");
    await settle();
    const page = card(view.host, "日程");

    await click(button(await planned(page, "火车"), "改…"));
    // 从现在这一条填起，时间是这台电脑的本地时间。
    expect(text(page)).toContain("修改日程");
    expect(field(page, "标题").value).toBe("火车");
    expect(field(page, "开始时间").value).toBe("2026-11-26T09:00");
    expect(field(page, "结束时间").value).toBe("2026-11-26T10:00");
    expect(field(page, "重复").value).toBe("none");
    expect(field(page, "提醒").value).toBe("30");
    expect(field(page, "地点").value).toBe("上海虹桥站");
    expect(field(page, "备注").value).toBe("带身份证");

    await fill(page, "标题", "高铁");
    await fill(page, "重复", "daily");
    await fill(page, "结束时间", "2026-11-26T09:00");
    await click(button(page, "保存"));
    expect(text(page)).toContain("结束时间要晚于开始时间。");
    expect(source.calls).toEqual([]);

    // 结束时间不填也可以。
    await fill(page, "结束时间", "");
    await click(button(page, "保存"));
    expect(source.calls).toEqual(["scheduleUpdate:schedule-2:3:高铁"]);
    expect(source.lastSubmitted).toEqual({
      title: "高铁",
      startAt: utc(2026, 11, 26, 9),
      endAt: null,
      repeat: "daily",
      repeatUntil: null,
      remindBefore: 30,
      notes: "带身份证",
      location: { label: "上海虹桥站", address: "申虹路 1500 号" },
    });
  });

  test("改了地点：导航用的地址跟着换成新写的那个", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", planner: listing() }),
    );
    const view = await renderApp(source, "日程与提醒");
    await settle();
    const page = card(view.host, "日程");

    await click(button(await planned(page, "火车"), "改…"));
    await fill(page, "地点", "上海站");
    await click(button(page, "保存"));
    expect(source.lastSubmitted).toMatchObject({
      location: { label: "上海站", address: "上海站" },
    });
  });

  test("助手定下的提前量不在常选的那几个里：照样列着，不改就不会丢", async () => {
    const schedules = listing().schedules.map((schedule) =>
      schedule.id === "schedule-2"
        ? { ...schedule, remindBefore: 45 }
        : schedule,
    );
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", planner: listing({ schedules }) }),
    );
    const view = await renderApp(source, "日程与提醒");
    await settle();
    const page = card(view.host, "日程");

    expect(text(await planned(page, "火车"))).toContain("提前 45 分钟");
    await click(button(await planned(page, "火车"), "改…"));
    const lead = field(page, "提醒");
    expect(lead.value).toBe("45");
    expect(text(lead)).toContain("提前 45 分钟");
    await click(button(page, "保存"));
    expect(source.lastSubmitted).toMatchObject({ remindBefore: 45 });
  });

  test("别处改过：说明并收起表单，不回显技术细节", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", planner: listing() }),
    );
    source.failWith = new BridgeError(
      "VERSION_CONFLICT",
      "version 3 is stale; the record is at 4",
    );
    const view = await renderApp(source, "日程与提醒");
    await settle();
    const page = card(view.host, "日程");

    await click(button(await planned(page, "火车"), "改…"));
    await fill(page, "标题", "高铁");
    await click(button(page, "保存"));
    expect(text(view.host)).toContain(
      "这一条在别的设备上被改过。已经换成最新的，请再看一眼。",
    );
    expect(text(view.host)).not.toContain("version 3");
    // 表单里留着的是旧值：收起来，让人看最新的名单。
    expect(button(card(view.host, "日程"), "新建日程…")).toBeDefined();
    expect(button(card(view.host, "日程"), "保存")).toBeUndefined();
  });

  test("Rust 没放过去的内容：说明没有保存，表单留着", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", planner: listing() }),
    );
    source.failWith = new BridgeError(
      "PLANNER_INPUT",
      "the schedule content is not acceptable",
    );
    const view = await renderApp(source, "日程与提醒");
    await settle();
    const page = card(view.host, "日程");

    await click(button(page, "新建日程…"));
    await fill(page, "标题", "周会");
    await fill(page, "开始时间", "2026-11-26T09:30");
    await click(button(page, "保存"));
    expect(text(view.host)).toContain("有内容不合规则，没有保存。");
    expect(text(view.host)).not.toContain("not acceptable");
    expect(button(card(view.host, "日程"), "保存")).toBeDefined();
  });

  test("删除正在改的那一条要先确认；新建的表单没有这一项", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", planner: listing() }),
    );
    const view = await renderApp(source, "日程与提醒");
    await settle();
    let page = card(view.host, "日程");

    await click(button(await planned(page, "给妈妈打电话"), "改…"));
    await click(button(page, "删除…"));
    await click(button(page, "算了"));
    expect(source.calls).toEqual([]);

    await click(button(page, "删除…"));
    await click(button(page, "确认删除"));
    expect(source.calls).toEqual(["reminderDelete:reminder-2:1"]);

    page = card(view.host, "日程");
    await click(button(page, "新建日程…"));
    expect(button(page, "删除…")).toBeUndefined();
  });

  test("从托盘的“新建提醒…”进来：摆出表单，收起时告诉 Rust", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        planner: listing({ compose: "reminder" }),
      }),
    );
    const view = await renderApp(source, "日程与提醒");
    await settle();
    let page = card(view.host, "日程");
    expect(text(page)).toContain("新建提醒");
    expect(button(page, "新建日程…")).toBeUndefined();

    await fill(page, "标题", "取快递");
    await fill(page, "到点时间", "2026-11-26T09:05");
    await click(button(page, "保存"));
    expect(source.calls).toEqual([
      "reminderCreate:取快递",
      "plannerCompose:null",
    ]);

    // Rust 那边还没把"要摆表单"收掉就又推来一份名单：表单不会自己再弹出来。
    await act(async () => {
      source.push(
        snapshot({
          phase: "signedIn",
          planner: listing({ compose: "reminder" }),
        }),
      );
    });
    page = card(view.host, "日程");
    expect(button(page, "新建日程…")).toBeDefined();
    expect(button(page, "保存")).toBeUndefined();
  });

  test("托盘摆出的表单直接取消：也算收起了", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        planner: listing({ compose: "reminder" }),
      }),
    );
    const view = await renderApp(source, "日程与提醒");
    await settle();
    const page = card(view.host, "日程");
    await click(button(page, "取消"));
    expect(source.calls).toEqual(["plannerCompose:null"]);
    expect(button(card(view.host, "日程"), "保存")).toBeUndefined();
  });
});

describe("到点提醒", () => {
  test("开关反映这台电脑的实际设置；点开关只把要改成什么交给 Rust", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    const view = await renderApp(source, "本机设置");
    await settle();
    const toggle = switchNamed(view.host, "到点时弹提醒");
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    expect(text(view.host)).toContain("这台电脑会弹系统通知。");

    await click(toggle);
    expect(source.calls).toEqual(["setPlannerMuted:true"]);
    // 结果由 Rust 经状态推送回来，界面不自行翻转。
    expect(toggle.getAttribute("aria-checked")).toBe("true");

    await act(async () => {
      source.push(snapshot({ phase: "signedIn", plannerMuted: true }));
    });
    expect(
      switchNamed(view.host, "到点时弹提醒").getAttribute("aria-checked"),
    ).toBe("false");
    expect(text(view.host)).toContain("这台电脑不弹提醒");
  });

  test("没登录时没有这一项", async () => {
    const view = await renderApp(new FakeBridge(snapshot()), "本机设置");
    await settle();
    expect(text(view.host)).not.toContain("到点提醒");
    expect(view.host.querySelector('[aria-label="到点时弹提醒"]')).toBeNull();
  });
});
