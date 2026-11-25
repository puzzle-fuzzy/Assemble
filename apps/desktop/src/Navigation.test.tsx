// 验证图标导航、账户浮层与托盘入口，防止拆分主窗口后丢失原有操作。
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act } from "react";
import {
  button,
  card,
  cleanup,
  click,
  field,
  fill,
  settle,
  text,
} from "@/test-support/dom";
import { FakeBridge, PLANNER, snapshot } from "@/test-support/fakes";
import { renderApp } from "@/test-support/render-app";

beforeEach(() => document.body.replaceChildren());
afterEach(cleanup);

describe("主窗口导航", () => {
  test("默认只显示传输，图标有名称和唯一选中项，点击后切换内容", async () => {
    const view = await renderApp(
      new FakeBridge(snapshot({ phase: "signedIn" })),
    );
    const current = () => view.host.querySelector('[aria-current="page"]');
    expect(current()?.getAttribute("aria-label")).toBe("文件传输");
    expect(text(view.host)).not.toContain("季度报表.xlsx");
    expect(view.host.querySelector('[data-slot="popover-content"]')).toBeNull();
    for (const name of ["文件传输", "云盘", "日程与提醒", "本机设置"]) {
      const trigger = button(view.host, name);
      expect(trigger?.textContent).toBe("");
      await click(trigger);
      expect(current()).toBe(trigger ?? null);
      expect(view.host.querySelectorAll('[aria-current="page"]')).toHaveLength(
        1,
      );
      expect(
        view.host
          .querySelector("section[id^=view-]:not([hidden])")
          ?.getAttribute("aria-label"),
      ).toBe(name);
    }
    await click(button(view.host, "云盘"));
    expect(text(view.host)).toContain("季度报表.xlsx");
    expect(text(view.host)).not.toContain("还没有收发过文件。");
  });

  test("账户和设备只在点头像后出现，关闭后保持原来选择的内容", async () => {
    const view = await renderApp(
      new FakeBridge(snapshot({ phase: "signedIn" })),
      "云盘",
    );
    expect(text(view.host)).not.toContain("这台设备");
    expect(button(document.body, "退出登录")).toBeUndefined();
    await click(button(view.host, "账户"));
    const panel = document.body.querySelector<HTMLElement>(
      '[data-slot="popover-content"]',
    );
    expect(panel).not.toBeNull();
    expect(text(panel as HTMLElement)).toContain("这台设备：MacBook-Pro");
    expect(card(panel as HTMLElement, "设备")).toBeDefined();
    await click(button(document.body, "关闭账户面板"));
    expect(button(document.body, "退出登录")).toBeUndefined();
    expect(
      view.host
        .querySelector('[aria-current="page"]')
        ?.getAttribute("aria-label"),
    ).toBe("云盘");
  });

  test("提醒草稿在页面之间切换后仍保留", async () => {
    const view = await renderApp(
      new FakeBridge(snapshot({ phase: "signedIn" })),
      "日程与提醒",
    );
    await click(button(card(view.host, "日程"), "新建提醒…"));
    await fill(view.host, "标题", "周五带上合同");
    await fill(view.host, "备注", "先给同事确认");
    await click(button(view.host, "云盘"));
    expect(button(view.host, "保存")).toBeUndefined();
    await click(button(view.host, "日程与提醒"));
    expect(field(view.host, "标题").value).toBe("周五带上合同");
    expect(field(view.host, "备注").value).toBe("先给同事确认");
  });

  test("托盘和通知会切到日程，切走后同一个入口仍可再次定位", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    const view = await renderApp(source, "云盘");
    await act(async () =>
      source.push({
        ...source.current,
        planner: { ...PLANNER, compose: "reminder" },
      }),
    );
    expect(button(view.host, "保存")).toBeDefined();
    expect(
      view.host
        .querySelector('[aria-current="page"]')
        ?.getAttribute("aria-label"),
    ).toBe("日程与提醒");
    await click(button(view.host, "文件传输"));
    expect(source.calls).toContain("plannerCompose:null");
    // Rust 确认清理后，再收到相同的托盘事件。
    await act(async () =>
      source.push({ ...source.current, planner: { ...PLANNER } }),
    );
    await act(async () =>
      source.push({
        ...source.current,
        planner: { ...PLANNER, compose: "reminder" },
      }),
    );
    expect(button(view.host, "保存")).toBeDefined();
    await act(async () =>
      source.push({
        ...source.current,
        planner: { ...PLANNER, focus: "reminder-1" },
      }),
    );
    await click(button(view.host, "云盘"));
    expect(source.calls).toContain("plannerFocus:null");
    await act(async () =>
      source.push({ ...source.current, planner: { ...PLANNER } }),
    );
    await act(async () =>
      source.push({
        ...source.current,
        planner: { ...PLANNER, focus: "reminder-1" },
      }),
    );
    expect(
      view.host
        .querySelector('[aria-current="page"]')
        ?.getAttribute("aria-label"),
    ).toBe("日程与提醒");
  });

  test("浏览器完成登录后收起账户浮层", async () => {
    const source = new FakeBridge(snapshot({ phase: "signingIn" }));
    const view = await renderApp(source, "账户");
    expect(text(view.host)).toContain("完成后会自动返回");
    await act(async () => source.push(snapshot({ phase: "signedIn" })));
    await settle();
    expect(button(document.body, "退出登录")).toBeUndefined();
    expect(text(view.host)).toContain("还没有收发过文件。");
  });
});
