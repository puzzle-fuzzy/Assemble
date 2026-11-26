// 发送面板的测试：挑好的文件、账户里的设备、发出去之后的进度与结果。

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act } from "react";
import { BridgeError } from "@/lib/bridge";
import type { Snapshot, TransferView } from "@/lib/types";
import { SendPanel } from "@/SendPanel";
import {
  button,
  cleanup,
  click,
  render,
  settle,
  text,
  wait,
} from "@/test-support/dom";
import {
  DEVICE,
  FakeBridge,
  OTHER,
  snapshot,
  transfer,
} from "@/test-support/fakes";

/** 面板里"发往哪里"的全部去处：云盘，加上账户里的设备。 */
function destinations(host: HTMLElement): HTMLButtonElement[] {
  return [
    ...host.querySelectorAll<HTMLButtonElement>("ul[aria-label] > li > button"),
  ];
}

/** 其中属于设备的那几个（不是云盘）。 */
function devices(host: HTMLElement): HTMLButtonElement[] {
  return destinations(host).slice(1);
}

/** 模拟 Rust 推来新的状态。 */
async function push(source: FakeBridge, next: Snapshot) {
  await act(async () => {
    source.push(next);
  });
}

beforeEach(() => {
  document.body.replaceChildren();
});

afterEach(cleanup);

describe("发送面板", () => {
  test("没登录时请用户先去主窗口登录", async () => {
    const source = new FakeBridge(snapshot());
    const view = await render(<SendPanel bridge={source} />);
    await settle();

    expect(text(view.host)).toContain(
      "请先在 Assemble 主窗口登录，再发送文件。",
    );
    await click(button(view.host, "打开 Assemble"));
    expect(source.calls).toEqual(["openMain"]);
  });

  test("没有挑文件时如实说，并能重新选择或关闭", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    const view = await render(<SendPanel bridge={source} />);
    await settle();

    expect(text(view.host)).toContain("没有要发送的文件。");
    await click(button(view.host, "重新选择"));
    await click(button(view.host, "关闭"));
    expect(source.calls).toEqual(["startSend", "closeSendPanel"]);
  });

  test("列出挑好的文件与账户里的别的设备，离线的不能选", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        draft: {
          id: "draft-1",
          files: [
            { name: "季度报表.xlsx", size: 524288 },
            { name: "说明.txt", size: 2048 },
          ],
        },
        devices: [
          { ...DEVICE },
          { ...OTHER, online: true },
          {
            id: "device-3",
            name: "家里的台式机",
            platform: "linux",
            online: false,
          },
        ],
      }),
    );
    const view = await render(<SendPanel bridge={source} />);
    await settle();

    const page = text(view.host);
    expect(page).toContain("季度报表.xlsx");
    expect(page).toContain("512 KB");
    expect(page).toContain("说明.txt");
    expect(page).toContain("2.0 KB");
    const selectedFiles = view.host.querySelector('ul[aria-label="已选文件"]');
    expect(selectedFiles).not.toBeNull();
    expect(selectedFiles?.querySelector("button, a, [tabindex]")).toBeNull();
    // 云盘排在最前面，永远能选；本机不在设备名单里；离线的设备点不了。
    const choices = destinations(view.host);
    expect(choices.map((choice) => choice.textContent)).toEqual([
      "上传到云盘",
      "发送到 工作电脑Windows · 在线",
      "发送到 家里的台式机Linux · 离线",
    ]);
    expect(choices[0].disabled).toBe(false);
    expect(choices[1].disabled).toBe(false);
    expect(choices[2].disabled).toBe(true);
    expect(page).not.toContain("MacBook-Pro");
  });

  test("手机第一期收不了文件：不列成目标，并说一声为什么", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        draft: {
          id: "draft-1",
          files: [{ name: "季度报表.xlsx", size: 524288 }],
        },
        devices: [
          { ...DEVICE },
          { ...OTHER, online: true },
          {
            id: "device-4",
            name: "我的 iPhone",
            platform: "ios",
            online: false,
          },
          {
            id: "device-5",
            name: "将来的设备",
            platform: "other",
            online: true,
          },
        ],
      }),
    );
    const view = await render(<SendPanel bridge={source} />);
    await settle();

    // 云盘和能收文件的电脑照旧；手机和认不出平台的设备都不在名单里。
    const choices = destinations(view.host);
    expect(choices.map((choice) => choice.textContent)).toEqual([
      "上传到云盘",
      "发送到 工作电脑Windows · 在线",
    ]);
    expect(text(view.host)).toContain("手机第一期还不能收文件");
  });

  test("点「云盘」就把文件存进云盘，存好后自动收起", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        draft: {
          id: "draft-1",
          files: [{ name: "季度报表.xlsx", size: 524288 }],
        },
      }),
    );
    source.sentAs = ["t9"];
    const view = await render(<SendPanel bridge={source} />);
    await settle();

    await click(destinations(view.host)[0]);
    expect(source.calls).toEqual(["sendToDrive"]);

    const stored = (phase: TransferView["phase"], progress: number) =>
      transfer({
        id: "t9",
        direction: "out",
        target: "drive",
        peer: "云盘",
        name: "季度报表.xlsx",
        size: 524288,
        phase,
        progress,
      });
    await push(source, {
      ...snapshot({ phase: "signedIn" }),
      transfers: [stored("inProgress", 0.5)],
    });
    expect(text(view.host)).toContain("正在上传到云盘 50%");

    await push(source, {
      ...snapshot({ phase: "signedIn" }),
      transfers: [stored("complete", 1)],
    });
    expect(text(view.host)).toContain("已存到云盘");
    await wait(1700);
    expect(source.calls).toContain("closeSendPanel");
  });

  test("点一台在线设备就发过去，传完后自动收起", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        draft: {
          id: "draft-1",
          files: [{ name: "季度报表.xlsx", size: 524288 }],
        },
        devices: [{ ...DEVICE }, { ...OTHER, online: true }],
      }),
    );
    source.sentAs = ["t1"];
    const view = await render(<SendPanel bridge={source} />);
    await settle();

    await click(devices(view.host)[0]);
    expect(source.calls).toEqual(["sendToDevice:device-2"]);

    // Rust 那边开始上传了。
    const sent = transfer({
      id: "t1",
      direction: "out",
      phase: "inProgress",
      progress: 0.4,
    });
    await push(source, {
      ...snapshot({ phase: "signedIn" }),
      transfers: [sent],
    });
    expect(text(view.host)).toContain("正在发送 40%");
    expect(text(view.host)).toContain("传完之后会自动关闭。");
    expect(source.calls).not.toContain("closeSendPanel");

    // 文件到了服务端：停一会儿，然后收起。
    await push(source, {
      ...snapshot({ phase: "signedIn" }),
      transfers: [{ ...sent, phase: "waiting", progress: 1 }],
    });
    expect(text(view.host)).toContain("已发出，等对方接收");
    expect(text(view.host)).toContain("已发送");
    await wait(1700);
    expect(source.calls).toContain("closeSendPanel");
  });

  test("没发出去的留着让用户决定：可以重发，也可以关掉", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        draft: {
          id: "draft-1",
          files: [{ name: "季度报表.xlsx", size: 524288 }],
        },
        devices: [{ ...DEVICE }, { ...OTHER, online: true }],
      }),
    );
    source.sentAs = ["t1"];
    const view = await render(<SendPanel bridge={source} />);
    await settle();
    await click(devices(view.host)[0]);
    await push(source, {
      ...snapshot({ phase: "signedIn" }),
      transfers: [
        transfer({
          id: "t1",
          direction: "out",
          phase: "failed",
          failure: "deviceOffline",
          retryable: true,
        }),
      ],
    });

    expect(text(view.host)).toContain("没有全部传完");
    expect(text(view.host)).toContain("发送失败：那台设备刚刚下线了。");
    expect(button(view.host, "重发没发出去的")).toBeDefined();
    await wait(1700);
    expect(source.calls).not.toContain("closeSendPanel");

    await click(button(view.host, "重发没发出去的"));
    expect(source.calls).toContain("retryTransfer:t1");
    await click(button(view.host, "关闭"));
    expect(source.calls).toContain("closeSendPanel");
  });

  test("点下去才发现那台设备下线了：说明原因，仍停在设备名单上", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        draft: {
          id: "draft-1",
          files: [{ name: "季度报表.xlsx", size: 524288 }],
        },
        devices: [{ ...DEVICE }, { ...OTHER, online: true }],
      }),
    );
    source.failWith = new BridgeError("DEVICE_OFFLINE", "target is offline");
    const view = await render(<SendPanel bridge={source} />);
    await settle();

    await click(devices(view.host)[0]);
    expect(text(view.host)).toContain("那台设备刚刚下线了，没法发送。");
    expect(devices(view.host)).toHaveLength(1);
  });
});
