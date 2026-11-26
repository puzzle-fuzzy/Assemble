// 验证文字导航、账户中心与托盘入口，防止更换布局后丢失原有操作。
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
import {
  ACCOUNT,
  DEVICE,
  FakeBridge,
  PLANNER,
  snapshot,
} from "@/test-support/fakes";
import {
  navigateAccount,
  openAccountMenu,
  renderApp,
} from "@/test-support/render-app";

beforeEach(() => document.body.replaceChildren());
afterEach(cleanup);

describe("主窗口导航", () => {
  test("默认显示传输，菜单展示文字与唯一选中项，点击后切换整页内容", async () => {
    const view = await renderApp(
      new FakeBridge(snapshot({ phase: "signedIn" })),
    );
    const current = () => view.host.querySelector('[aria-current="page"]');
    expect(current()?.getAttribute("aria-label")).toBe("文件传输");
    expect(text(view.host)).not.toContain("季度报表.xlsx");
    expect(view.host.querySelector('[data-slot="popover-content"]')).toBeNull();
    for (const name of ["文件传输", "云盘", "日程与提醒", "本机设置"]) {
      const trigger = button(view.host, name);
      expect(trigger?.querySelector("span")?.textContent).toBe(name);
      await click(trigger);
      expect(current()).toBe(trigger ?? null);
      expect(
        view.host.querySelectorAll(
          'nav[aria-label="主功能"] [aria-current="page"]',
        ),
      ).toHaveLength(1);
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

  test("侧栏底部显示当前身份，账户菜单进入账户中心并将焦点交给页面", async () => {
    const view = await renderApp(
      new FakeBridge(snapshot({ phase: "signedIn" })),
      "云盘",
    );
    expect(text(view.host)).not.toContain("这台设备");
    expect(button(view.host, "退出登录")).toBeUndefined();
    const trigger = button(view.host, "账户菜单");
    expect(trigger?.closest('[data-slot="sidebar-footer"]')).not.toBeNull();
    expect(trigger?.textContent).toContain(ACCOUNT.displayName);
    expect(
      button(
        view.host.querySelector('nav[aria-label="主功能"]') as HTMLElement,
        "账户中心",
      ),
    ).toBeUndefined();
    await navigateAccount(view.host);
    expect(text(card(view.host, "账户"))).toContain("这台设备：MacBook-Pro");
    expect(card(view.host, "设备")).toBeDefined();
    expect(document.activeElement).toBe(
      view.host.querySelector('header [data-slot="breadcrumb-page"]'),
    );
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
    expect(
      view.host.querySelector('[data-slot="popover-content"]') === null,
    ).toBe(true);
    await click(button(view.host, "云盘"));
    expect(button(view.host, "退出登录")).toBeUndefined();
    expect(
      view.host
        .querySelector('nav [aria-current="page"]')
        ?.getAttribute("aria-label"),
    ).toBe("云盘");
  });

  test("账户菜单关闭回到触发器；退出失败保留身份与草稿，进行中只发一次命令", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    let reject: ((error: unknown) => void) | undefined;
    source.signOut = async () => {
      source.calls.push("signOut");
      return new Promise<typeof source.current>((_, fail) => {
        reject = fail;
      });
    };
    const view = await renderApp(source, "日程与提醒");
    await click(button(card(view.host, "日程"), "新建提醒"));
    await fill(view.host, "标题", "退出失败仍需保留");
    const trigger = await openAccountMenu(view.host);
    await act(async () => {
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });
    await settle();
    expect(document.activeElement).toBe(trigger);
    await openAccountMenu(view.host);
    const item = [
      ...document.body.querySelectorAll<HTMLElement>('[role="menuitem"]'),
    ].find((candidate) => candidate.textContent === "退出登录");
    await click(item);
    // 退出期间仍可打开外观菜单，但不能重复发起账户操作。
    expect(trigger.disabled).toBe(false);
    await openAccountMenu(view.host);
    const pendingLogout = [
      ...document.body.querySelectorAll<HTMLElement>('[role="menuitem"]'),
    ].find((candidate) => candidate.textContent === "退出登录");
    expect(pendingLogout?.getAttribute("aria-disabled")).toBe("true");
    await click(pendingLogout);
    expect(source.calls.filter((call) => call === "signOut")).toHaveLength(1);
    await act(async () =>
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      ),
    );
    await act(async () => reject?.({ code: "SERVICE_UNAVAILABLE" }));
    await settle();
    expect(trigger.disabled).toBe(false);
    expect(trigger.textContent).toContain(ACCOUNT.displayName);
    expect(field(view.host, "标题").value).toBe("退出失败仍需保留");
    expect(view.host.querySelector('[role="alert"]')).not.toBeNull();
  });

  test("提醒草稿在页面之间切换后仍保留", async () => {
    const view = await renderApp(
      new FakeBridge(snapshot({ phase: "signedIn" })),
      "日程与提醒",
    );
    await click(button(card(view.host, "日程"), "新建提醒"));
    await fill(view.host, "标题", "周五带上合同");
    await fill(view.host, "备注", "先给同事确认");
    await click(button(view.host, "云盘"));
    expect(button(view.host, "保存")).toBeUndefined();
    await click(button(view.host, "日程与提醒"));
    expect(field(view.host, "标题").value).toBe("周五带上合同");
    expect(field(view.host, "备注").value).toBe("先给同事确认");
  });

  test.each([false, true])(
    "账户更替后不显示上一账户的提醒草稿（经过退出：%s）",
    async (signedOut) => {
      const source = new FakeBridge(snapshot({ phase: "signedIn" }));
      const view = await renderApp(source, "日程与提醒");
      await click(button(card(view.host, "日程"), "新建提醒"));
      await fill(view.host, "标题", "原账户的私人草稿");
      if (signedOut) await act(async () => source.push(snapshot()));
      await act(async () =>
        source.push({
          ...snapshot({ phase: "signedIn" }),
          session: {
            phase: "signedIn",
            account: { ...ACCOUNT, id: "account-2" },
            device: { ...DEVICE, id: "device-2" },
          },
        }),
      );
      expect(button(view.host, "保存") === undefined).toBe(true);
      await click(button(card(view.host, "日程"), "新建提醒"));
      expect(field(view.host, "标题").value).toBe("");
    },
  );

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

  test("浏览器完成登录后账户中心显示当前账户与设备", async () => {
    const source = new FakeBridge(snapshot({ phase: "signingIn" }));
    const view = await renderApp(source, "账户");
    expect(text(view.host)).toContain("完成后会自动返回");
    await act(async () => source.push(snapshot({ phase: "signedIn" })));
    await settle();
    expect(button(view.host, "退出登录")).toBeDefined();
    expect(text(view.host)).toContain("这台设备：MacBook-Pro");
  });
});
