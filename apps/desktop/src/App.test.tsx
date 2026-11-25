// 界面测试：注入替身桥接，在真实 DOM 里验证用户看到的与触发的操作。

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act } from "react";
import { toast } from "sonner";

import { App } from "@/App";
import { BridgeError } from "@/lib/bridge";
import type { TransferView } from "@/lib/types";
import {
  button,
  buttons,
  card,
  cleanup,
  click,
  launchSwitch,
  press,
  render,
  settle,
  text,
  textField,
  typeInto,
} from "@/test-support/dom";
import {
  ACCOUNT,
  DEVICE,
  DRIVE,
  FakeBridge,
  OTHER,
  service,
  signedIn,
  snapshot,
  transfer,
} from "@/test-support/fakes";

beforeEach(() => {
  document.body.replaceChildren();
});

afterEach(async () => {
  toast.dismiss();
  await cleanup();
});

describe("版本信息", () => {
  test("固定在所有内容卡片之后，未登录、已登录或断网时都显示构建身份", async () => {
    for (const current of [
      snapshot(),
      snapshot({ phase: "signedIn" }),
      snapshot({ service: service({ reachable: false }) }),
    ]) {
      const view = await render(<App bridge={new FakeBridge(current)} />);
      await settle();
      const footer = view.host.querySelector("footer");
      expect(footer?.querySelector("p")?.textContent).toBe(
        "本地测试 - 0.1.0.1234567",
      );
      expect(footer?.querySelectorAll("p")[1]?.textContent).toBe(
        "官网：http://127.0.0.1:8787",
      );
      expect(view.host.querySelector("main")?.lastElementChild).toBe(footer);
      await view.unmount();
    }
  });
});

describe("连接状态", () => {
  test("探测期间提示检测，正常连接后不再显示含糊的服务类型", async () => {
    const source = new FakeBridge(
      snapshot({
        service: service({ target: "development", reachable: null }),
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();
    expect(text(view.host)).toContain("正在检测连接");

    await act(async () => {
      source.push(
        snapshot({
          service: service({ target: "development", reachable: true }),
        }),
      );
    });
    expect(text(view.host)).not.toContain("本地开发服务");
    expect(text(view.host)).not.toContain("正式服务");
    expect(text(view.host)).not.toContain("正在检测连接");
    expect(text(view.host)).not.toContain("暂不可达");
  });

  test("服务不可达时如实说明", async () => {
    const source = new FakeBridge(
      snapshot({
        service: service({ target: "production", reachable: false }),
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();
    expect(text(view.host)).toContain("暂时无法连接服务器");
  });

  test("登录后显示事件流的连接情况", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        service: service({ stream: "connecting" }),
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();
    // 事件流还在建立：不能说成"已连接"。
    expect(text(view.host)).toContain("正在连接…");

    await act(async () => {
      source.push(
        snapshot({
          phase: "signedIn",
          service: service({ stream: "connected" }),
        }),
      );
    });
    expect(text(view.host)).toContain("已连接");
    expect(text(view.host)).not.toContain("正在连接");

    await act(async () => {
      source.push(
        snapshot({
          phase: "signedIn",
          service: service({ stream: "reconnecting" }),
        }),
      );
    });
    expect(text(view.host)).toContain("连接已断开，正在重连");

    // 连服务都探不到：同一句话，不再说成服务名不可达。
    await act(async () => {
      source.push(
        snapshot({ phase: "signedIn", service: service({ reachable: false }) }),
      );
    });
    expect(text(view.host)).toContain("连接已断开，正在重连");
    expect(text(view.host)).not.toContain("正式服务暂不可达");
  });
});

describe("账户", () => {
  test("未登录时可以发起登录，进行中不重复触发", async () => {
    const source = new FakeBridge(snapshot());
    const view = await render(<App bridge={source} />);
    await settle();

    const signIn = button(view.host, "使用浏览器登录");
    if (!signIn) throw new Error("没有找到登录按钮");
    // 让登录命令一直挂着，断言完再放行：不靠零延时定时器和界面的重渲染赛跑。
    source.hold();
    await press(signIn);
    await settle();
    expect(source.calls).toEqual(["signIn"]);
    // 命令还没返回：不再显示可重复点击的按钮。
    expect(button(view.host, "使用浏览器登录")).toBeUndefined();
    source.release();
    await settle();
    expect(button(view.host, "使用浏览器登录")).toBeDefined();
    // 整个过程只触发了一次登录。
    expect(source.calls).toEqual(["signIn"]);
  });

  test("服务不可达时不能登录，并说明原因", async () => {
    const source = new FakeBridge(
      snapshot({
        service: service({ target: "production", reachable: false }),
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();

    expect(button(view.host, "使用浏览器登录")?.disabled).toBe(true);
    expect(text(view.host)).toContain("服务暂不可达，恢复后即可登录。");
    expect(source.calls).toEqual([]);
  });

  test("登录进行中可以取消", async () => {
    const source = new FakeBridge(snapshot({ phase: "signingIn" }));
    const view = await render(<App bridge={source} />);
    await settle();

    expect(text(view.host)).toContain(
      "已在浏览器打开登录页，完成后会自动返回。",
    );
    await click(button(view.host, "取消"));
    expect(source.calls).toEqual(["cancelSignIn"]);
  });

  test("已登录时显示账户并可以退出", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    const view = await render(<App bridge={source} />);
    await settle();

    expect(text(view.host)).toContain("Example（example）");
    await click(button(view.host, "退出登录"));
    expect(source.calls).toEqual(["signOut"]);
  });

  test("显示名与用户名相同时不重复显示", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    source.current = {
      ...source.current,
      session: signedIn({ ...ACCOUNT, displayName: "example" }),
    };
    const view = await render(<App bridge={source} />);
    await settle();
    expect(text(view.host)).toContain("example");
    expect(text(view.host)).not.toContain("（example）");
  });

  test("已登录时显示这台设备在账户里的名字", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    const view = await render(<App bridge={source} />);
    await settle();

    expect(text(view.host)).toContain("这台设备：MacBook-Pro");

    // 设备在别的设备上被改名后，随状态推送更新。
    await act(async () => {
      source.push({
        ...snapshot({ phase: "signedIn" }),
        session: {
          phase: "signedIn",
          account: ACCOUNT,
          device: { ...DEVICE, name: "家里的 Mac" },
        },
        devices: [{ ...DEVICE, name: "家里的 Mac" }],
      });
    });
    expect(text(view.host)).toContain("这台设备：家里的 Mac");
    expect(text(view.host)).not.toContain("这台设备：MacBook-Pro");
  });

  test("命令失败时显示固定文案，不回显技术细节", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    source.failWith = new BridgeError(
      "UNREACHABLE",
      "connect tcp 10.0.0.1:3300: refused",
    );
    const view = await render(<App bridge={source} />);
    await settle();

    await click(button(view.host, "退出登录"));
    expect(text(view.host)).toContain("暂时连不上服务，请检查网络后重试。");
    expect(text(view.host)).not.toContain("10.0.0.1");
  });
});

describe("提示", () => {
  test("登录超时只在 toast 中提示，状态重复推送时不叠加", async () => {
    const source = new FakeBridge(
      snapshot({
        notice: { kind: "error", message: "等待浏览器登录超时，请重试。" },
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();

    const timeout = "等待浏览器登录超时，请重试。";
    expect(
      view.host.querySelector("[data-sonner-toast]")?.textContent,
    ).toContain(timeout);
    expect(view.host.querySelector("main")?.textContent).not.toContain(timeout);
    expect(
      view.host
        .querySelector('[aria-live="polite"]')
        ?.contains(view.host.querySelector("[data-sonner-toast]")),
    ).toBe(true);

    await act(async () => {
      source.push({ ...source.current });
    });
    await settle();
    expect(view.host.querySelectorAll("[data-sonner-toast]")).toHaveLength(1);
  });

  test("关闭超时 toast 会通知 Rust，重试后的超时仍能再次提示", async () => {
    const timeout = {
      kind: "error",
      message: "等待浏览器登录超时，请重试。",
    } as const;
    const source = new FakeBridge(snapshot({ notice: timeout }));
    const view = await render(<App bridge={source} />);
    await settle();

    const close = view.host.querySelector<HTMLButtonElement>(
      'button[aria-label="关闭提示"]',
    );
    if (!close) throw new Error("没有找到 toast 关闭按钮");
    await click(close);
    expect(source.calls).toEqual(["dismissNotice"]);

    await act(async () => {
      source.push(snapshot({ notice: null }));
    });
    await act(async () => {
      source.push(snapshot({ notice: timeout }));
    });
    await settle();
    expect(view.host.querySelectorAll("[data-sonner-toast]")).toHaveLength(1);
    expect(
      view.host.querySelector("[data-sonner-toast]")?.textContent,
    ).toContain(timeout.message);
  });

  test("登录超时的命令拒绝不再往页面叠加通用错误", async () => {
    const source = new FakeBridge(snapshot());
    source.signIn = async () => {
      source.push(
        snapshot({
          notice: { kind: "error", message: "等待浏览器登录超时，请重试。" },
        }),
      );
      throw new BridgeError("LOGIN_CALLBACK_FAILED", "login timed out");
    };
    const view = await render(<App bridge={source} />);
    await settle();
    await click(button(view.host, "使用浏览器登录"));
    expect(
      view.host.querySelector("[data-sonner-toast]")?.textContent,
    ).toContain("等待浏览器登录超时，请重试。");
    expect(view.host.querySelector("main")?.textContent).not.toContain(
      "操作没有完成",
    );
    expect(view.host.querySelector("main")?.textContent).not.toContain(
      "等待浏览器登录超时",
    );
  });

  test("其他错误提示沿用页面原有呈现", async () => {
    const source = new FakeBridge(
      snapshot({
        notice: { kind: "error", message: "请先登录，再发送文件。" },
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();
    expect(
      view.host.querySelector('main [role="alert"]')?.textContent,
    ).toContain("请先登录，再发送文件。");
    expect(button(view.host, "知道了")).toBeDefined();
  });

  test("点「知道了」告诉 Rust 收起提示", async () => {
    const source = new FakeBridge(
      snapshot({
        notice: { kind: "info", message: "登录已失效，请重新登录。" },
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();

    await click(button(view.host, "知道了"));
    expect(source.calls).toEqual(["dismissNotice"]);
  });

  test("随后的状态推送会替换掉旧提示", async () => {
    const source = new FakeBridge(
      snapshot({
        notice: { kind: "info", message: "登录已失效，请重新登录。" },
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();
    expect(text(view.host)).toContain("登录已失效，请重新登录。");

    await act(async () => {
      source.push(snapshot({ notice: null }));
    });
    expect(text(view.host)).not.toContain("登录已失效，请重新登录。");
  });
});

describe("开机启动", () => {
  test("开关反映系统里的实际状态，点击后才请求修改", async () => {
    const source = new FakeBridge(snapshot({ launchAtLogin: "disabled" }));
    const view = await render(<App bridge={source} />);
    await settle();

    expect(launchSwitch(view.host).getAttribute("aria-checked")).toBe("false");
    expect(source.calls).toEqual([]);

    await click(launchSwitch(view.host));
    expect(source.calls).toEqual(["setLaunchAtLogin:true"]);
    // 读回的结果由 Rust 经状态推送回来，界面不自行翻转。
    expect(launchSwitch(view.host).getAttribute("aria-checked")).toBe("false");

    await act(async () => {
      source.push(snapshot({ launchAtLogin: "enabled" }));
    });
    expect(launchSwitch(view.host).getAttribute("aria-checked")).toBe("true");
  });

  test("不支持时禁用并说明原因", async () => {
    const source = new FakeBridge(snapshot({ launchAtLogin: "unsupported" }));
    const view = await render(<App bridge={source} />);
    await settle();

    expect(launchSwitch(view.host).disabled).toBe(true);
    expect(text(view.host)).toContain("开发运行不设置开机启动。");
  });

  test("等待系统批准时不显示为已开启", async () => {
    const source = new FakeBridge(snapshot({ launchAtLogin: "unknown" }));
    const view = await render(<App bridge={source} />);
    await settle();

    expect(launchSwitch(view.host).getAttribute("aria-checked")).toBe("false");
    expect(text(view.host)).toContain("等待系统批准后生效。");
  });

  test("系统拒绝修改时显示提示", async () => {
    const source = new FakeBridge(snapshot({ launchAtLogin: "disabled" }));
    source.failWith = new BridgeError("SYSTEM_REJECTED", "os error 5");
    const view = await render(<App bridge={source} />);
    await settle();

    await click(launchSwitch(view.host));
    expect(text(view.host)).toContain("系统没有接受这个设置，请稍后重试。");
    expect(text(view.host)).not.toContain("os error");
  });
});

describe("传输", () => {
  test("没登录时没有这一块；登录后没有记录时如实说还没有", async () => {
    const signedOut = await render(<App bridge={new FakeBridge(snapshot())} />);
    await settle();
    expect(text(signedOut.host)).not.toContain("传输");

    const view = await render(
      <App bridge={new FakeBridge(snapshot({ phase: "signedIn" }))} />,
    );
    await settle();
    expect(text(view.host)).toContain("还没有收发过文件。");
  });

  test("每条记录说清楚是谁发来的、多大的文件、现在怎么样", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        transfers: [
          transfer({
            id: "t4",
            name: "视频.mp4",
            phase: "failed",
            progress: 0,
            failure: "diskFull",
          }),
          transfer({
            id: "t3",
            name: "报表.xlsx",
            phase: "cancelled",
            progress: 0,
          }),
          transfer({
            id: "t2",
            name: "会议录音.m4a",
            phase: "expired",
            progress: 0,
          }),
          transfer({
            id: "t5",
            name: "合同.pdf",
            phase: "queued",
            progress: 0,
          }),
          transfer({
            id: "t6",
            name: "照片.jpg",
            phase: "inProgress",
            progress: 0.452,
          }),
          transfer({ id: "t1", savedAs: "合同 (2).pdf" }),
        ],
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();

    const page = text(view.host);
    expect(page).toContain("合同.pdf");
    expect(page).toContain("工作电脑 · 1.5 MB");
    expect(page).toContain("已存为「合同 (2).pdf」");
    expect(page).toContain("正在接收 45%");
    expect(page).toContain("等待接收…");
    expect(page).toContain("已保存");
    expect(page).toContain("接收失败：这台电脑的磁盘已满。");
    expect(page).toContain("对方撤回了，没有收到。");
    expect(page).toContain("过了时限，没有收到。");
    // 失败以警示的方式读出来。
    expect(view.host.querySelectorAll('[role="alert"]')).toHaveLength(1);
    expect(page).not.toContain("原因不明");
  });

  test("每一种失败原因都有自己的说法", async () => {
    for (const [failure, words] of [
      ["writeFailed", "文件没能写进接收文件夹"],
      ["verificationFailed", "收到的内容与发出的对不上"],
      ["cancelled", "已取消"],
    ] as const) {
      document.body.replaceChildren();
      const source = new FakeBridge(
        snapshot({
          phase: "signedIn",
          transfers: [transfer({ phase: "failed", progress: 0, failure })],
        }),
      );
      const view = await render(<App bridge={source} />);
      await settle();
      expect(text(view.host)).toContain(`接收失败：${words}。`);
      await view.unmount();
    }
  });

  test("只有收好的文件能要求显示，而且只交出传输的标识", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        transfers: [
          transfer(),
          transfer({ id: "t2", phase: "inProgress", progress: 0.3 }),
          transfer({
            id: "t3",
            phase: "failed",
            progress: 0,
            failure: "writeFailed",
          }),
        ],
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();

    expect(buttons(view.host, "在文件夹中显示")).toHaveLength(1);
    await click(button(view.host, "在文件夹中显示"));
    expect(source.calls).toEqual(["revealReceived:t1"]);
  });

  test("文件已经不在时说明情况，不回显技术细节", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", transfers: [transfer()] }),
    );
    source.failWith = new BridgeError("NOT_SAVED", "no file for t1 on disk");
    const view = await render(<App bridge={source} />);
    await settle();

    await click(button(view.host, "在文件夹中显示"));
    expect(text(view.host)).toContain("这个文件还没有保存到这台电脑上。");
    expect(text(view.host)).not.toContain("t1 on disk");
  });

  test("进度随状态推送更新，完成后出现显示按钮", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        transfers: [transfer({ phase: "inProgress", progress: 0.2 })],
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();
    expect(text(view.host)).toContain("正在接收 20%");
    expect(button(view.host, "在文件夹中显示")).toBeUndefined();

    await act(async () => {
      source.push(snapshot({ phase: "signedIn", transfers: [transfer()] }));
    });
    expect(text(view.host)).toContain("已保存");
    expect(button(view.host, "在文件夹中显示")).toBeDefined();
  });

  test("发出的传输按方向措辞；能取消的给「取消」，能重发的才给「重发」", async () => {
    const outgoing = (
      id: string,
      phase: TransferView["phase"],
      extra: Partial<TransferView> = {},
    ) => transfer({ id, direction: "out", phase, progress: 0, ...extra });
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        transfers: [
          outgoing("t8", "queued"),
          outgoing("t7", "inProgress", { progress: 0.3 }),
          outgoing("t6", "waiting", { progress: 1 }),
          outgoing("t5", "complete", { progress: 1 }),
          outgoing("t4", "declined"),
          outgoing("t3", "cancelled"),
          outgoing("t2", "expired"),
          outgoing("t1", "failed", {
            failure: "deviceOffline",
            retryable: true,
          }),
          outgoing("t0", "failed", { failure: "writeFailed" }),
        ],
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();

    const page = text(view.host);
    expect(page).toContain("发给 工作电脑 · 1.5 MB");
    expect(page).toContain("等待发送…");
    expect(page).toContain("正在发送 30%");
    expect(page).toContain("已发出，等对方接收");
    expect(page).toContain("对方已收下");
    expect(page).toContain("对方拒收了。");
    expect(page).toContain("已取消。");
    expect(page).toContain("对方没有在时限内接收。");
    expect(page).toContain("发送失败：那台设备刚刚下线了。");
    expect(page).toContain("发送失败：对方设备没能把文件写下来。");
    // 还没到头的三条可以取消；只有文件没到服务端的那次失败可以重发。
    // 日程的每一行上也有"取消"：只在传输卡片里数。
    expect(buttons(card(view.host, "传输"), "取消")).toHaveLength(3);
    expect(buttons(card(view.host, "传输"), "重发")).toHaveLength(1);
    // 发出去的文件不在这台电脑上，没有"在文件夹中显示"。
    expect(buttons(card(view.host, "传输"), "在文件夹中显示")).toHaveLength(0);
  });

  test("取消与重发只交出传输的标识", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        transfers: [
          transfer({
            id: "t2",
            direction: "out",
            phase: "waiting",
            progress: 1,
          }),
          transfer({
            id: "t1",
            direction: "out",
            phase: "failed",
            failure: "deviceOffline",
            retryable: true,
          }),
        ],
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();

    // 两个按钮各属于各自的那条记录。
    const transfers = card(view.host, "传输");
    await click(button(transfers, "取消"));
    await click(button(transfers, "重发"));
    expect(source.calls).toEqual(["cancelTransfer:t2", "retryTransfer:t1"]);
  });

  test("点「发送文件…」交给系统选择器；挑的文件读不出来时说明原因", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    const view = await render(<App bridge={source} />);
    await settle();

    await click(button(view.host, "发送文件…"));
    expect(source.calls).toEqual(["startSend"]);

    source.failWith = new BridgeError("FILE_UNREADABLE", "open /x/y: EACCES");
    await click(button(view.host, "发送文件…"));
    expect(text(view.host)).toContain(
      "有文件读不出来，可能已被移走或没有权限。",
    );
    expect(text(view.host)).not.toContain("EACCES");
  });
});

describe("云盘", () => {
  test("列出云盘里的文件与已用空间；没登录时没有这一块", async () => {
    const signedOut = await render(<App bridge={new FakeBridge(snapshot())} />);
    await settle();
    expect(text(signedOut.host)).not.toContain("云盘");

    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    const view = await render(<App bridge={source} />);
    await settle();
    const page = text(view.host);
    expect(page).toContain("已用 1.5 MB，共 2.0 GB");
    expect(page).toContain("季度报表.xlsx");
    expect(page).toContain("512 KB · 2026-11-24");
    expect(page).toContain("合同.pdf");
    expect(source.calls).toEqual([]);
  });

  test("名单还没拿回来时向 Rust 要一次，不反复要", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn", drive: null }));
    const view = await render(<App bridge={source} />);
    await settle();
    expect(source.calls).toEqual(["driveRefresh"]);
    expect(text(view.host)).toContain("正在读取云盘…");

    // 还是没拿到：不重复要。
    await act(async () => {
      source.push(snapshot({ phase: "signedIn", drive: null }));
    });
    await settle();
    expect(source.calls).toEqual(["driveRefresh"]);
  });

  test("上传交给系统选择器；取回只交出文件的标识", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    const view = await render(<App bridge={source} />);
    await settle();

    await click(button(view.host, "上传…"));
    await click(button(view.host, "取回"));
    expect(source.calls).toEqual(["driveUpload", "driveDownload:file-1"]);
  });

  test("删除要先确认，取消就什么都不做", async () => {
    const source = new FakeBridge(snapshot({ phase: "signedIn" }));
    const view = await render(<App bridge={source} />);
    await settle();

    await click(button(view.host, "删除"));
    expect(source.calls).toEqual([]);
    expect(button(card(view.host, "云盘"), "确认删除")).toBeDefined();

    await click(button(card(view.host, "云盘"), "取消"));
    expect(source.calls).toEqual([]);
    expect(button(card(view.host, "云盘"), "确认删除")).toBeUndefined();

    await click(button(view.host, "删除"));
    await click(button(view.host, "确认删除"));
    expect(source.calls).toEqual(["driveDelete:file-1"]);
  });

  test("存进云盘、从云盘取回，各说各的", async () => {
    const toDrive = (extra: Partial<TransferView>) =>
      transfer({
        direction: "out",
        target: "drive",
        peer: "云盘",
        progress: 0,
        ...extra,
      });
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        transfers: [
          toDrive({ id: "t5", phase: "queued" }),
          toDrive({ id: "t4", phase: "inProgress", progress: 0.6 }),
          toDrive({ id: "t3", phase: "complete", progress: 1 }),
          toDrive({
            id: "t2",
            phase: "failed",
            failure: "quotaExceeded",
            retryable: true,
          }),
          transfer({
            id: "t1b",
            target: "drive",
            peer: "云盘",
            name: "旧资料.pdf",
            phase: "failed",
            failure: "vanished",
          }),
          transfer({
            id: "t1",
            target: "drive",
            peer: "云盘",
            name: "合同.pdf",
          }),
        ],
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();
    const page = text(view.host);
    expect(page).toContain("等待上传…");
    expect(page).toContain("正在上传到云盘 60%");
    expect(page).toContain("已存到云盘");
    expect(page).toContain("没能存进云盘：云盘空间不够。");
    expect(page).toContain("取回失败：云盘里已经没有这个文件了。");
    expect(page).toContain("已取回");
    // 存进云盘的文件不经过接收位置：只有取回来的那个能"在文件夹中显示"。
    expect(buttons(view.host, "在文件夹中显示")).toHaveLength(1);
    // 空间不够的可以重发；云盘里已经没有的不能。
    expect(buttons(view.host, "重发")).toHaveLength(1);
  });

  test("云盘是空的时候如实说", async () => {
    const source = new FakeBridge(
      snapshot({
        phase: "signedIn",
        drive: { ...DRIVE, items: [], usedBytes: 0 },
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();
    expect(text(view.host)).toContain("云盘里还没有文件。");
    expect(text(view.host)).toContain("已用 0 B，共 2.0 GB");
  });
});

describe("右键菜单", () => {
  test("不是 Windows 的打包运行时没有这一项", async () => {
    const source = new FakeBridge(snapshot({ sendMenu: "unsupported" }));
    const view = await render(<App bridge={source} />);
    await settle();
    expect(text(view.host)).not.toContain("右键菜单");
    expect(text(view.host)).toContain("接收位置");
  });

  test("开着时说明它在哪儿，点开关只把要改成什么交给 Rust", async () => {
    const source = new FakeBridge(snapshot({ sendMenu: "enabled" }));
    const view = await render(<App bridge={source} />);
    await settle();
    expect(text(view.host)).toContain("右键菜单");
    expect(text(view.host)).toContain(
      "在文件上点右键，选择「用 Assemble 发送」。",
    );
    expect(text(view.host)).not.toContain("显示更多选项");
    const toggle = view.host.querySelector<HTMLButtonElement>(
      '[aria-label="右键菜单里的“用 Assemble 发送”"]',
    );
    if (!toggle) throw new Error("没有找到右键菜单开关");
    expect(toggle.getAttribute("aria-checked")).toBe("true");

    await click(toggle);
    expect(source.calls).toEqual(["setSendMenu:false"]);
    // 结果由 Rust 经状态推送回来，界面不自行翻转。
    expect(toggle.getAttribute("aria-checked")).toBe("true");
  });

  test("关掉时如实说菜单里没有它", async () => {
    const source = new FakeBridge(snapshot({ sendMenu: "disabled" }));
    const view = await render(<App bridge={source} />);
    await settle();
    expect(text(view.host)).toContain(
      "文件的右键菜单里没有「用 Assemble 发送」",
    );
  });

  test("系统不让改时显示固定文案", async () => {
    const source = new FakeBridge(snapshot({ sendMenu: "disabled" }));
    source.failWith = new BridgeError("SYSTEM_REJECTED", "os error 5");
    const view = await render(<App bridge={source} />);
    await settle();
    const toggle = view.host.querySelector<HTMLButtonElement>(
      '[aria-label="右键菜单里的“用 Assemble 发送”"]',
    );
    if (!toggle) throw new Error("没有找到右键菜单开关");
    await click(toggle);
    expect(text(view.host)).toContain("系统没有接受这个设置，请稍后重试。");
    expect(text(view.host)).not.toContain("os error");
  });
});

describe("接收位置", () => {
  test("默认是下载/Assemble，点「更改」由系统选择器来挑", async () => {
    const source = new FakeBridge(snapshot());
    const view = await render(<App bridge={source} />);
    await settle();

    expect(text(view.host)).toContain("下载/Assemble");
    expect(button(view.host, "恢复默认")).toBeUndefined();

    await click(button(view.host, "更改…"));
    expect(source.calls).toEqual(["chooseReceiveFolder"]);
  });

  test("选了文件夹之后只显示它的名字，可以恢复默认", async () => {
    const source = new FakeBridge(
      snapshot({ receive: { customFolder: "工作资料", displayPath: null } }),
    );
    const view = await render(<App bridge={source} />);
    await settle();

    expect(text(view.host)).toContain("「工作资料」文件夹");
    expect(text(view.host)).not.toContain("下载/Assemble");

    await click(button(view.host, "恢复默认"));
    expect(source.calls).toEqual(["resetReceiveFolder"]);
    // 结果由 Rust 经状态推送回来，界面不自行改写。
    expect(text(view.host)).toContain("「工作资料」文件夹");

    await act(async () => {
      source.push(snapshot());
    });
    expect(text(view.host)).toContain("下载/Assemble");
    expect(button(view.host, "恢复默认")).toBeUndefined();
  });

  test("选的文件夹不能用时说明原因", async () => {
    const source = new FakeBridge(snapshot());
    source.failWith = new BridgeError("INVALID_RECEIVE_FOLDER", "NotDirectory");
    const view = await render(<App bridge={source} />);
    await settle();

    await click(button(view.host, "更改…"));
    expect(text(view.host)).toContain(
      "这个文件夹不能用来保存收到的文件，请换一个。",
    );
    expect(text(view.host)).not.toContain("NotDirectory");
  });

  test("Windows 展示系统给出的完整目录，更改和恢复默认后跟随状态更新", async () => {
    const defaultPath = "D:\\系统下载\\Assemble";
    const source = new FakeBridge(
      snapshot({ receive: { customFolder: null, displayPath: defaultPath } }),
    );
    const view = await render(<App bridge={source} />);
    await settle();
    expect(text(card(view.host, "本机"))).toContain(defaultPath);
    expect(text(card(view.host, "本机"))).not.toContain("下载/Assemble");
    expect(button(view.host, "恢复默认")).toBeUndefined();

    const customPath = "E:\\工作资料\\本年度归档资料\\收件文件夹";
    await act(async () => {
      source.push(
        snapshot({
          receive: { customFolder: "收件文件夹", displayPath: customPath },
        }),
      );
    });
    const location = [...view.host.querySelectorAll("p")].find(
      (p) => p.textContent === customPath,
    );
    expect(location?.classList.contains("break-all")).toBe(true);
    expect(location?.classList.contains("truncate")).toBe(false);
    await click(button(view.host, "恢复默认"));
    expect(source.calls).toEqual(["resetReceiveFolder"]);

    await act(async () => {
      source.push(
        snapshot({ receive: { customFolder: null, displayPath: defaultPath } }),
      );
    });
    expect(text(card(view.host, "本机"))).toContain(defaultPath);
    expect(button(view.host, "恢复默认")).toBeUndefined();
  });
});

describe("订阅生命周期", () => {
  test("卸载后不再接收状态", async () => {
    const source = new FakeBridge(snapshot());
    const view = await render(<App bridge={source} />);
    await settle();
    expect(source.listenerCount).toBe(1);

    await view.unmount();
    expect(source.listenerCount).toBe(0);
    await act(async () => {
      source.push(snapshot({ phase: "signedIn" }));
    });
    expect(text(view.host)).toBe("");
  });
});

describe("设备", () => {
  test("列出账户里的全部设备，标出本机与在线情况", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", devices: [{ ...DEVICE }, { ...OTHER }] }),
    );
    const view = await render(<App bridge={source} />);
    await settle();

    const page = text(view.host);
    expect(page).toContain("MacBook-Pro");
    expect(page).toContain("本机");
    expect(page).toContain("工作电脑");
    expect(page).toContain("Windows · 离线");
    expect(page).toContain("Mac · 在线");
    // 本机不能被改名或移除；别的设备可以。
    expect(buttons(view.host, "改名")).toHaveLength(1);
    expect(buttons(view.host, "移除")).toHaveLength(1);
  });

  test("没登录时不显示设备，哪怕状态里残留着设备", async () => {
    const source = new FakeBridge(snapshot({ devices: [{ ...OTHER }] }));
    const view = await render(<App bridge={source} />);
    await settle();
    expect(text(view.host)).not.toContain("工作电脑");
    expect(text(view.host)).not.toContain("设备");
  });

  test("别的设备上线或下线时跟着状态推送更新", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", devices: [{ ...DEVICE }, { ...OTHER }] }),
    );
    const view = await render(<App bridge={source} />);
    await settle();
    expect(text(view.host)).toContain("Windows · 离线");

    await act(async () => {
      source.push(
        snapshot({
          phase: "signedIn",
          devices: [{ ...DEVICE }, { ...OTHER, online: true }],
        }),
      );
    });
    expect(text(view.host)).toContain("Windows · 在线");
  });

  test("给别的设备改名", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", devices: [{ ...DEVICE }, { ...OTHER }] }),
    );
    const view = await render(<App bridge={source} />);
    await settle();

    await click(button(view.host, "改名"));
    // 从当前名字开始改。
    expect(textField(view.host).value).toBe("工作电脑");
    await typeInto(view.host, "公司的电脑");
    await click(button(view.host, "保存"));

    expect(source.calls).toEqual(["renameDevice:device-2:公司的电脑"]);
  });

  test("名字没变就不发请求，空名字在本地拦下", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", devices: [{ ...DEVICE }, { ...OTHER }] }),
    );
    const view = await render(<App bridge={source} />);
    await settle();

    await click(button(view.host, "改名"));
    await click(button(view.host, "保存"));
    expect(source.calls).toEqual([]);
    expect(button(view.host, "改名")).toBeDefined();

    await click(button(view.host, "改名"));
    await typeInto(view.host, "   ");
    await click(button(view.host, "保存"));
    expect(source.calls).toEqual([]);
    expect(text(view.host)).toContain("名字不能为空");
    // 还停在编辑状态，可以直接改。
    expect(textField(view.host)).toBeDefined();
  });

  test("移除别的设备要先确认", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", devices: [{ ...DEVICE }, { ...OTHER }] }),
    );
    const view = await render(<App bridge={source} />);
    await settle();

    await click(button(view.host, "移除"));
    // 只是把按钮换成确认，还没有发请求。
    expect(source.calls).toEqual([]);
    await click(button(card(view.host, "设备"), "取消"));
    expect(button(card(view.host, "设备"), "移除")).toBeDefined();

    await click(button(view.host, "移除"));
    await click(button(view.host, "确认移除"));
    expect(source.calls).toEqual(["removeDevice:device-2"]);
  });

  test("改动失败时显示固定文案，不回显技术细节", async () => {
    const source = new FakeBridge(
      snapshot({ phase: "signedIn", devices: [{ ...DEVICE }, { ...OTHER }] }),
    );
    source.failWith = new BridgeError("NOT_FOUND", "devices/abc row not found");
    const view = await render(<App bridge={source} />);
    await settle();

    await click(button(view.host, "移除"));
    await click(button(view.host, "确认移除"));
    expect(text(view.host)).toContain("这台设备已经不在账户里了。");
    expect(text(view.host)).not.toContain("row not found");
  });
});
