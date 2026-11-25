// 云盘直接展示 Rust 的上传状态，不能等上传完成后才出现文件。
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act } from "react";
import {
  button,
  card,
  cleanup,
  click,
  press,
  settle,
  text,
  typeInto,
} from "@/test-support/dom";
import {
  ACCOUNT,
  DEVICE,
  DRIVE,
  FakeBridge,
  snapshot,
  transfer,
} from "@/test-support/fakes";
import { renderApp } from "@/test-support/render-app";

beforeEach(() => document.body.replaceChildren());
afterEach(cleanup);

const upload = (
  phase: "queued" | "inProgress" | "failed" | "complete" | "cancelled",
  progress = 0,
) =>
  transfer({
    id: "upload-1",
    name: "会议录像.mp4",
    target: "drive",
    direction: "out",
    peer: "云盘",
    size: 60 * 1024 * 1024,
    phase,
    progress,
  });
const emptyDrive = { ...DRIVE, items: [], usedBytes: 0 };

describe("云盘上传进度", () => {
  test("选择和准备文件时立即反馈，同一次选择不重复发起", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", drive: emptyDrive }),
    );
    source.hold();
    const view = await renderApp(source, "云盘");
    const trigger = button(view.host, "上传…");
    try {
      await press(trigger);
      expect(button(view.host, "正在准备…")?.disabled).toBe(true);
      await press(trigger);
      expect(source.calls).toEqual(["driveUpload"]);
    } finally {
      await act(async () => source.release());
      await settle();
    }
    expect(button(view.host, "上传…")?.disabled).toBe(false);
  });

  test("排队即展示文件，实际百分比持续更新，搜索和页面切换不隐藏上传", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", drive: emptyDrive }),
    );
    const view = await renderApp(source, "云盘");
    await click(button(view.host, "上传…"));
    await act(async () =>
      source.push({ ...source.current, transfers: [upload("queued")] }),
    );
    const page = card(view.host, "云盘");
    const progress = () => page.querySelector('[role="progressbar"]');
    expect(text(page)).toContain("会议录像.mp4");
    expect(text(page)).toContain("等待上传…");
    expect(text(page)).not.toContain("云盘里还没有文件");
    expect(progress()?.getAttribute("aria-valuenow")).toBe("0");
    expect(progress()?.getAttribute("aria-label")).toBe(
      "会议录像.mp4的传输进度",
    );
    for (const fraction of [0.23, 0.68]) {
      await act(async () =>
        source.push({
          ...source.current,
          transfers: [upload("inProgress", fraction)],
        }),
      );
      expect(progress()?.getAttribute("aria-valuenow")).toBe(
        String(Math.floor(fraction * 100)),
      );
      expect(text(page)).toContain(
        `正在上传到云盘 ${Math.floor(fraction * 100)}%`,
      );
    }
    await typeInto(view.host, "其他文件");
    expect(progress()).not.toBeNull();
    await click(button(view.host, "日程与提醒"));
    await click(button(view.host, "云盘"));
    expect(progress()?.getAttribute("aria-valuenow")).toBe("68");
    await click(button(page, "取消"));
    expect(source.calls).toEqual(["driveUpload", "cancelTransfer:upload-1"]);
    await act(async () =>
      source.push({ ...source.current, transfers: [upload("cancelled")] }),
    );
    expect(progress()).toBeNull();
  });

  test("失败显示真实原因并可重试，成功后转为云盘文件且不抢焦点", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        drive: emptyDrive,
        transfers: [
          { ...upload("failed"), failure: "quotaExceeded", retryable: true },
        ],
      }),
    );
    const view = await renderApp(source, "云盘");
    const page = card(view.host, "云盘");
    expect(text(page)).toContain("没能存进云盘：云盘空间不够。");
    await click(button(page, "重发"));
    expect(source.calls).toEqual(["retryTransfer:upload-1"]);
    await act(async () =>
      source.push({
        ...source.current,
        transfers: [upload("inProgress", 0.8)],
      }),
    );
    const trigger = button(view.host, "上传…");
    trigger?.focus();
    await act(async () =>
      source.push({
        ...source.current,
        transfers: [upload("complete", 1)],
        drive: {
          ...emptyDrive,
          items: [
            {
              id: "stored-1",
              name: "会议录像.mp4",
              size: 60 * 1024 * 1024,
              createdAt: "2026-11-25T01:00:00Z",
            },
          ],
          usedBytes: 60 * 1024 * 1024,
        },
      }),
    );
    expect(page.querySelector('[role="progressbar"]')).toBeNull();
    expect(page.querySelector('[role="status"]')?.textContent).toBe(
      "会议录像.mp4上传完成",
    );
    expect(text(page)).toContain("60.0 MB / 2.0 GB");
    expect(button(page, "取回")).toBeDefined();
    expect(document.activeElement).toBe(trigger ?? null);
  });

  test("退出或更换账户后不显示旧账户的上传任务", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", transfers: [upload("inProgress", 0.5)] }),
    );
    const view = await renderApp(source, "云盘");
    expect(text(card(view.host, "云盘"))).toContain("会议录像.mp4");
    await act(async () => source.push(snapshot()));
    expect(text(view.host)).not.toContain("会议录像.mp4");
    await act(async () =>
      source.push({
        ...snapshot({ phase: "signedIn", drive: emptyDrive }),
        session: {
          phase: "signedIn",
          account: { ...ACCOUNT, id: "other-account" },
          device: DEVICE,
        },
      }),
    );
    expect(text(card(view.host, "云盘"))).not.toContain("会议录像.mp4");
    expect(
      card(view.host, "云盘").querySelector('[role="progressbar"]'),
    ).toBeNull();
  });
});
