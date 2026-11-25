// 列表与详情分开验证，避免仅有视觉上的三列而操作仍落到整页名单。
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
    window as unknown as Window & {
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

async function select(host: HTMLElement, id: string) {
  await click(
    host.querySelector<HTMLElement>(`[data-item-id="${id}"]`) ?? undefined,
  );
}

describe("sidebar-09 列表与详情", () => {
  test("传输列表选中哪条，详情和桥接操作就只针对哪条", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        transfers: [
          transfer({ id: "saved", name: "已收到.pdf" }),
          transfer({
            id: "sending",
            name: "正在发.zip",
            direction: "out",
            phase: "waiting",
          }),
        ],
      }),
    );
    const view = await renderApp(source);
    expect(text(card(view.host, "传输"))).toContain("已收到.pdf");
    expect(text(card(view.host, "传输"))).not.toContain("正在发.zip");
    await select(view.host, "sending");
    const detail = card(view.host, "传输");
    expect(text(detail)).toContain("正在发.zip");
    expect(text(detail)).not.toContain("已收到.pdf");
    expect(button(detail, "在文件夹中显示")).toBeUndefined();
    await click(button(detail, "取消"));
    expect(source.calls).toEqual(["cancelTransfer:sending"]);
    expect(
      view.host
        .querySelector('[data-item-id="sending"]')
        ?.getAttribute("aria-current"),
    ).toBe("true");
  });

  test("云盘选择文件后才显示对应操作，删除确认不会沿用到另一文件", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    const view = await renderApp(source, "云盘");
    await click(button(card(view.host, "云盘"), "删除"));
    await select(view.host, "file-2");
    const detail = card(view.host, "云盘");
    expect(text(detail)).toContain("合同.pdf");
    expect(text(detail)).not.toContain("季度报表.xlsx");
    expect(button(detail, "确认删除")).toBeUndefined();
    await click(button(detail, "取回"));
    expect(source.calls).toEqual(["driveDownload:file-2"]);
  });

  test("搜索筛选真实列表，账户切换后清除关键词与旧选择", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    const view = await renderApp(source, "云盘");
    const list = () =>
      view.host.querySelector<HTMLElement>(
        '[aria-label="云盘列表"]',
      ) as HTMLElement;
    await typeInto(list(), "合同");
    expect(list().querySelectorAll("[data-item-id]").length).toBe(1);
    expect(text(card(view.host, "云盘"))).toContain("合同.pdf");
    await typeInto(list(), "查无此文件");
    expect(text(list())).toContain("没有匹配的项目");
    expect(text(card(view.host, "云盘"))).toContain("没有选中的文件");
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
    expect(list().querySelector("input")?.value).toBe("");
    expect(list().querySelectorAll("[data-item-id]").length).toBe(2);
  });

  test("收折列表后保留模块图标，展开仍能回到选中项", async () => {
    resize(960, 640);
    const view = await renderApp(
      new FakeBridge(snapshot({ phase: "signedIn" })),
      "云盘",
    );
    await select(view.host, "file-2");
    await click(button(view.host, "收折列表栏"));
    expect(view.host.querySelector('[aria-label="云盘列表"]') === null).toBe(
      true,
    );
    expect(button(view.host, "云盘")?.getAttribute("aria-current")).toBe(
      "page",
    );
    expect(text(card(view.host, "云盘"))).toContain("合同.pdf");
    await click(button(view.host, "收折列表栏"));
    expect(
      view.host
        .querySelector('[data-item-id="file-2"]')
        ?.getAttribute("aria-current"),
    ).toBe("true");
  });

  test("720×480 窗口始终有模块栏，展开中栏不切成整屏移动抽屉", async () => {
    resize(720, 480);
    const view = await renderApp(
      new FakeBridge(snapshot({ phase: "signedIn" })),
    );
    await settle();
    for (const name of ["文件传输", "云盘", "日程与提醒", "本机设置", "账户"])
      expect(button(view.host, name)).toBeDefined();
    await click(button(view.host, "云盘"));
    expect(view.host.querySelector('[aria-label="云盘列表"]') !== null).toBe(
      true,
    );
    expect(
      document.body.querySelector('[data-slot="sheet-content"]') === null,
    ).toBe(true);
    await select(view.host, "file-2");
    expect(text(card(view.host, "云盘"))).toContain("合同.pdf");
    await click(button(view.host, "收折列表栏"));
    expect(view.host.querySelector('[aria-label="云盘列表"]') === null).toBe(
      true,
    );
    expect(button(view.host, "本机设置")).toBeDefined();
  });

  test("通知能从托盘草稿切到对应详情，返回草稿仍保留输入", async () => {
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
    await select(view.host, "draft");
    expect(field(view.host, "标题").value).toBe("保存这份草稿");
  });
});
