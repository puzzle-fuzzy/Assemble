// 切换语言只改变系统界面，不重建用户草稿、查询或进行中的传输。

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { setLanguage, setTheme } from "@assemble/display";
import { act } from "react";
import { toast } from "sonner";
import { BridgeError } from "@/lib/bridge";
import en from "@/locales/en.json";
import zhCN from "@/locales/zh-CN.json";
import { SendPanel } from "@/SendPanel";
import {
  button,
  card,
  cleanup,
  click,
  field,
  fill,
  render,
  settle,
  text,
  typeInto,
} from "@/test-support/dom";
import {
  DEVICE,
  FakeBridge,
  OTHER,
  snapshot,
  transfer,
} from "@/test-support/fakes";
import { renderApp } from "@/test-support/render-app";

beforeEach(async () => {
  document.body.replaceChildren();
  await act(async () => {
    setLanguage("zh-CN");
    setTheme("system");
  });
});
afterEach(async () => {
  toast.dismiss();
  await cleanup();
  await act(async () => {
    setLanguage("zh-CN");
    setTheme("system");
  });
});

describe("桌面语言切换", () => {
  test("两份 UI 资源键和插值一致，英文系统文案不回退中文", () => {
    expect(Object.keys(en).sort()).toEqual(Object.keys(zhCN).sort());
    for (const key of Object.keys(zhCN) as (keyof typeof zhCN)[]) {
      expect(en[key]).not.toMatch(/[\u3400-\u9fff]/);
      expect(en[key].match(/\{\{[^}]+\}\}/g) ?? []).toEqual(
        zhCN[key].match(/\{\{[^}]+\}\}/g) ?? [],
      );
    }
  });

  test("当前页面、搜索和草稿随界面切换保留，文件名与日程内容不翻译", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    const view = await renderApp(source, "云盘");
    await typeInto(view.host, "合同");
    await act(async () => {
      setLanguage("en");
      setTheme("dark");
    });
    expect(button(view.host, "File transfers")).toBeDefined();
    expect(document.title).toBe("Cloud drive · Assemble");
    expect(document.documentElement.lang).toBe("en");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    const search = view.host.querySelector<HTMLInputElement>(
      '[aria-label="Search cloud drive"]',
    );
    expect(search?.value).toBe("合同");
    expect(search?.closest('[data-slot="card-header"]')).not.toBeNull();
    expect(text(card(view.host, "Cloud drive"))).toContain("合同.pdf");
    expect(text(card(view.host, "Cloud drive"))).not.toContain("季度报表.xlsx");
    await click(button(view.host, "Schedules and reminders"));
    await click(button(view.host, "New reminder…"));
    await fill(view.host, "Title", "明天带上合同");
    await fill(view.host, "Notes", "先找同事确认");
    await act(async () => setLanguage("zh-CN"));
    expect(field(view.host, "标题").value).toBe("明天带上合同");
    expect(field(view.host, "备注").value).toBe("先找同事确认");
    expect(button(view.host, "保存")).toBeDefined();
    expect(source.calls).toEqual([]);
  });

  test("已显示的命令与表单错误按新语言重新映射，不暴露诊断或修改输入", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    source.failWith = new BridgeError(
      "FILE_UNREADABLE",
      "private diagnostic path",
    );
    const view = await renderApp(source, "云盘");
    await click(button(view.host, "上传…"));
    expect(text(view.host)).toContain("有文件读不出来");
    await act(async () => setLanguage("en"));
    expect(text(view.host)).toContain("A file cannot be read.");
    expect(text(view.host)).not.toContain("private diagnostic");
    source.failWith = null;
    await click(button(view.host, "Schedules and reminders"));
    await click(button(view.host, "New reminder…"));
    await click(button(view.host, "Save"));
    expect(text(view.host)).toContain("Enter a title first.");
    await fill(view.host, "Notes", "不要翻译这段备注");
    await act(async () => setLanguage("zh-CN"));
    expect(text(view.host)).toContain("先写一个标题。");
    expect(field(view.host, "备注").value).toBe("不要翻译这段备注");
  });

  test("上传任务和百分比保留，完成提示跟随当前语言", async () => {
    const uploading = transfer({
      id: "i18n-upload",
      name: "用户录像.mp4",
      target: "drive",
      direction: "out",
      peer: "云盘",
      phase: "inProgress",
      progress: 0.46,
    });
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", transfers: [uploading] }),
    );
    const view = await renderApp(source, "云盘");
    await act(async () => setLanguage("en"));
    const page = card(view.host, "Cloud drive");
    expect(text(page)).toContain("用户录像.mp4");
    expect(text(page)).toContain("Uploading to cloud drive 46%");
    expect(
      page.querySelector('[role="progressbar"]')?.getAttribute("aria-valuenow"),
    ).toBe("46");
    expect(
      page.querySelector('[role="progressbar"]')?.getAttribute("aria-label"),
    ).toBe("Transfer progress for 用户录像.mp4");
    await act(async () =>
      source.push({
        ...source.current,
        transfers: [{ ...uploading, phase: "complete", progress: 1 }],
      }),
    );
    expect(page.querySelector('[role="status"]')?.textContent).toBe(
      "用户录像.mp4 uploaded",
    );
    await act(async () => setLanguage("zh-CN"));
    expect(page.querySelector('[role="status"]')?.textContent).toBe(
      "用户录像.mp4上传完成",
    );
  });

  test("已显示的登录超时 toast 原位换语言，不重复或提前回执", async () => {
    const source = new FakeBridge(
      snapshot({
        notice: { kind: "error", message: "等待浏览器登录超时，请重试。" },
      }),
    );
    await renderApp(source);
    expect(document.body.textContent).toContain("等待浏览器登录超时，请重试。");
    await act(async () => setLanguage("en"));
    await settle();
    expect(document.body.textContent).toContain(
      "Browser sign-in timed out. Please try again.",
    );
    expect(document.body.querySelectorAll("[data-sonner-toast]")).toHaveLength(
      1,
    );
    expect(source.calls).not.toContain("dismissNotice");
  });

  test("发送面板有共享外观入口，已显示的发送错误和标题跟随语言", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        draft: {
          id: "draft-i18n",
          files: [{ name: "用户的合同.pdf", size: 2048 }],
        },
        devices: [DEVICE, { ...OTHER, online: true }],
      }),
    );
    source.failWith = new BridgeError(
      "DEVICE_OFFLINE",
      "private service response",
    );
    const view = await render(<SendPanel bridge={source} />);
    const destination = view.host.querySelectorAll<HTMLButtonElement>(
      "ul[aria-label] > li > button",
    )[1];
    await click(destination);
    expect(text(view.host)).toContain("那台设备刚刚下线了");
    await act(async () => setLanguage("en"));
    expect(document.title).toBe("Send files · Assemble");
    expect(button(view.host, "Upload to cloud drive")).toBeDefined();
    expect(text(destination)).toContain("Send to 工作电脑");
    expect(button(view.host, "Language and appearance")).toBeDefined();
    expect(text(view.host)).toContain("The other device just went offline.");
    expect(text(view.host)).toContain("用户的合同.pdf");
    expect(text(view.host)).toContain("工作电脑");
    expect(text(view.host)).not.toContain("private service response");
  });
});
