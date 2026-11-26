// 与管理端一致的整页菜单，以及搜索、窄窗口和账户中心的业务回归。
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
  typeInto,
} from "@/test-support/dom";
import {
  ACCOUNT,
  DEVICE,
  FakeBridge,
  PLANNER,
  snapshot,
  transfer,
} from "@/test-support/fakes";
import { renderApp } from "@/test-support/render-app";

function resize(width: number, height: number) {
  (
    window as unknown as {
      happyDOM: {
        setWindowSize: (size: { width: number; height: number }) => void;
      };
    }
  ).happyDOM.setWindowSize({ width, height });
}

beforeEach(() => document.body.replaceChildren());
afterEach(async () => {
  await cleanup();
  resize(1024, 768);
});

describe("sidebar-13 整页导航", () => {
  test("左侧展示功能文字菜单和底部账户区，右侧整页列出传输记录", async () => {
    const view = await renderApp(
      new FakeBridge(
        snapshot({
          phase: "signedIn",
          transfers: [
            transfer({ name: "已收到.pdf" }),
            transfer({
              id: "sending",
              name: "正在发.zip",
              direction: "out",
              phase: "waiting",
            }),
          ],
        }),
      ),
    );
    const nav = view.host.querySelector(
      'nav[aria-label="主功能"]',
    ) as HTMLElement;
    for (const name of ["文件传输", "云盘", "日程与提醒", "本机设置"])
      expect(button(nav, name)?.textContent).toBe(name);
    expect(button(nav, "关于")).toBeUndefined();
    expect(button(nav, "关于软件")).toBeUndefined();
    expect(button(view.host, "账户菜单")).toBeDefined();
    expect(text(card(view.host, "传输"))).toContain("已收到.pdf");
    expect(text(card(view.host, "传输"))).toContain("正在发.zip");
    expect(view.host.querySelector("[data-item-id]") === null).toBe(true);
    expect(
      view.host.querySelector('[data-slot="sidebar-trigger"]') === null,
    ).toBe(true);
  });

  test("账户中心是菜单中的独立页面，包含登录和设备操作", async () => {
    const source = new FakeBridge(snapshot());
    const view = await renderApp(source);
    expect(button(view.host, "使用浏览器登录")).toBeUndefined();
    await click(button(view.host, "打开账户中心"));
    expect(
      view.host.querySelector('header [data-slot="breadcrumb-page"]')
        ?.textContent,
    ).toBe("账户中心");
    await click(button(view.host, "使用浏览器登录"));
    expect(source.calls).toEqual(["signIn"]);
    await act(async () => source.push(snapshot({ phase: "signedIn" })));
    expect(text(card(view.host, "账户"))).toContain("这台设备：MacBook-Pro");
    expect(card(view.host, "设备")).toBeDefined();
    expect(
      button(view.host.querySelector("header") as HTMLElement, "账户中心"),
    ).toBeUndefined();
  });

  test("搜索在内容区筛选，切换模块保留关键词，换账户后清空", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    const view = await renderApp(source, "云盘");
    await typeInto(view.host, "合同");
    expect(text(card(view.host, "云盘"))).toContain("合同.pdf");
    expect(text(card(view.host, "云盘"))).not.toContain("季度报表.xlsx");
    await click(button(view.host, "日程与提醒"));
    await click(button(view.host, "云盘"));
    expect(
      view.host.querySelector<HTMLInputElement>('[aria-label="搜索云盘"]')
        ?.value,
    ).toBe("合同");
    await typeInto(view.host, "查无此文件");
    expect(text(card(view.host, "云盘"))).toContain("没有匹配的文件");
    await act(async () =>
      source.push({
        ...source.current,
        session: {
          phase: "signedIn",
          account: { ...ACCOUNT, id: "new-account" },
          device: DEVICE,
        },
      }),
    );
    expect(
      view.host.querySelector<HTMLInputElement>('[aria-label="搜索云盘"]')
        ?.value,
    ).toBe("");
    expect(text(card(view.host, "云盘"))).toContain("季度报表.xlsx");
  });

  test("720×480 的功能菜单支持键盘打开，选择账户中心后焦点到达页面", async () => {
    resize(720, 480);
    const view = await renderApp(
      new FakeBridge(snapshot({ phase: "signedIn" })),
    );
    const trigger = button(view.host, "打开功能菜单");
    if (!trigger) throw new Error("缺少窄窗口菜单");
    await act(async () => {
      trigger.focus();
      trigger.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
      );
    });
    await settle();
    const item = [
      ...document.body.querySelectorAll<HTMLElement>('[role="menuitemradio"]'),
    ].find((candidate) => candidate.textContent === "账户中心");
    await click(item);
    expect(
      view.host.querySelector('header [data-slot="breadcrumb-page"]')
        ?.textContent,
    ).toBe("账户中心");
    expect(
      document.activeElement ===
        view.host.querySelector('header [data-slot="breadcrumb-page"]'),
    ).toBe(true);
    expect(document.body.querySelector('[role="menu"]') === null).toBe(true);
    expect(button(view.host, "退出登录")).toBeDefined();
  });

  test("通知定位暂时收起草稿，继续编辑后保留原有输入", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        planner: { ...PLANNER, compose: "reminder" },
      }),
    );
    const view = await renderApp(source);
    await fill(view.host, "标题", "保存这份草稿");
    await act(async () =>
      source.push({
        ...source.current,
        planner: { ...PLANNER, compose: "reminder", focus: "reminder-1" },
      }),
    );
    expect(text(card(view.host, "日程"))).toContain("驿站楼下");
    expect(button(view.host, "保存")).toBeUndefined();
    await click(button(view.host, "继续编辑"));
    expect(field(view.host, "标题").value).toBe("保存这份草稿");
  });
});
