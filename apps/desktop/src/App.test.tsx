// 界面测试：注入替身桥接，在真实 DOM 里验证用户看到的与触发的操作。

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";

import { App } from "@/App";
import { type Bridge, BridgeError, type UnlistenFn } from "@/lib/bridge";
import type { LaunchState, Snapshot } from "@/lib/types";

const ACCOUNT = {
  id: "account-1",
  username: "example",
  displayName: "Example",
};

type Overrides = Partial<Omit<Snapshot, "session">> & {
  phase?: Snapshot["session"]["phase"];
};

function snapshot(overrides: Overrides = {}): Snapshot {
  const { phase, ...rest } = overrides;
  const session: Snapshot["session"] =
    phase === "signingIn"
      ? { phase: "signingIn" }
      : phase === "signedIn"
        ? { phase: "signedIn", account: ACCOUNT }
        : { phase: "signedOut" };
  return {
    service: { target: "production", reachable: true },
    session,
    launchAtLogin: "disabled",
    notice: null,
    ...rest,
  };
}

/** 记录调用、可注入失败的替身桥接。 */
class FakeBridge implements Bridge {
  current: Snapshot;
  calls: string[] = [];
  failWith: BridgeError | null = null;
  pending = false;
  private listeners = new Set<(snapshot: Snapshot) => void>();

  constructor(initial: Snapshot) {
    this.current = initial;
  }

  private gate(): Promise<void> {
    this.pending = true;
    return new Promise((resolve) => {
      // 让调用方有机会观察到"进行中"的状态。
      setTimeout(() => {
        this.pending = false;
        resolve();
      }, 0);
    });
  }

  private async answer<T>(name: string, result: T): Promise<T> {
    this.calls.push(name);
    if (this.failWith) throw this.failWith;
    await this.gate();
    return result;
  }

  snapshot() {
    return Promise.resolve(this.current);
  }

  signIn() {
    return this.answer("signIn", this.current);
  }

  cancelSignIn() {
    return this.answer("cancelSignIn", undefined);
  }

  signOut() {
    return this.answer("signOut", this.current);
  }

  launchAtLogin() {
    return this.answer<LaunchState>(
      "launchAtLogin",
      this.current.launchAtLogin,
    );
  }

  setLaunchAtLogin(enabled: boolean) {
    this.calls.push(`setLaunchAtLogin:${enabled}`);
    if (this.failWith) throw this.failWith;
    return Promise.resolve(this.current.launchAtLogin);
  }

  async subscribe(listener: (snapshot: Snapshot) => void): Promise<UnlistenFn> {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  get listenerCount() {
    return this.listeners.size;
  }

  /** 模拟 Rust 推来新的状态。
   */
  push(next: Snapshot) {
    this.current = next;
    for (const listener of this.listeners) listener(next);
  }
}

interface Rendered {
  host: HTMLElement;
  unmount: () => Promise<void>;
}

const mounted: { root: Root; host: HTMLElement }[] = [];

async function render(ui: ReactElement): Promise<Rendered> {
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  mounted.push({ root, host });
  await act(async () => {
    root.render(ui);
  });
  return {
    host,
    unmount: async () => {
      await act(async () => {
        root.unmount();
      });
    },
  };
}

async function settle() {
  // 等待已排队的回调与零延时定时器跑完。
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 5));
  });
}

function text(host: HTMLElement) {
  return host.textContent ?? "";
}

function button(
  host: HTMLElement,
  name: string,
): HTMLButtonElement | undefined {
  return [...host.querySelectorAll("button")].find(
    (candidate) => candidate.textContent?.trim() === name,
  );
}

function launchSwitch(host: HTMLElement): HTMLButtonElement {
  const found = host.querySelector<HTMLButtonElement>('[role="switch"]');
  if (!found) throw new Error("没有找到开机启动开关");
  return found;
}

/** 点击但不等待命令返回：用来观察"进行中"的状态。 */
async function press(element: HTMLElement | undefined) {
  if (!element) throw new Error("要点击的元素不存在");
  await act(async () => {
    element.click();
  });
}

/** 点击并等命令返回、界面稳定。 */
async function click(element: HTMLElement | undefined) {
  await press(element);
  await settle();
}

beforeEach(() => {
  document.body.replaceChildren();
});

afterEach(async () => {
  for (const { root, host } of mounted.splice(0)) {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

describe("连接状态", () => {
  test("探测出结果前显示正在检测，之后显示服务名", async () => {
    const source = new FakeBridge(
      snapshot({ service: { target: "development", reachable: null } }),
    );
    const view = await render(<App bridge={source} />);
    await settle();
    expect(text(view.host)).toContain("正在检测连接");

    await act(async () => {
      source.push(
        snapshot({ service: { target: "development", reachable: true } }),
      );
    });
    expect(text(view.host)).toContain("本地开发服务");
    expect(text(view.host)).not.toContain("正在检测连接");
    expect(text(view.host)).not.toContain("暂不可达");
  });

  test("服务不可达时如实说明", async () => {
    const source = new FakeBridge(
      snapshot({ service: { target: "production", reachable: false } }),
    );
    const view = await render(<App bridge={source} />);
    await settle();
    expect(text(view.host)).toContain("正式服务暂不可达");
  });
});

describe("账户", () => {
  test("未登录时可以发起登录，进行中不重复触发", async () => {
    const source = new FakeBridge(snapshot());
    const view = await render(<App bridge={source} />);
    await settle();

    const signIn = button(view.host, "使用浏览器登录");
    if (!signIn) throw new Error("没有找到登录按钮");
    await press(signIn);
    expect(source.calls).toEqual(["signIn"]);
    // 命令还没返回：不再显示可重复点击的按钮。
    expect(button(view.host, "使用浏览器登录")).toBeUndefined();
    await settle();
    expect(button(view.host, "使用浏览器登录")).toBeDefined();
    // 整个过程只触发了一次登录。
    expect(source.calls).toEqual(["signIn"]);
  });

  test("服务不可达时不能登录，并说明原因", async () => {
    const source = new FakeBridge(
      snapshot({ service: { target: "production", reachable: false } }),
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
      session: {
        phase: "signedIn",
        account: { ...ACCOUNT, displayName: "example" },
      },
    };
    const view = await render(<App bridge={source} />);
    await settle();
    expect(text(view.host)).toContain("example");
    expect(text(view.host)).not.toContain("（example）");
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
  test("错误提示以警示方式出现", async () => {
    const source = new FakeBridge(
      snapshot({
        notice: { kind: "error", message: "等待浏览器登录超时，请重试。" },
      }),
    );
    const view = await render(<App bridge={source} />);
    await settle();

    const alert = view.host.querySelector('[role="alert"]');
    expect(alert?.textContent).toBe("等待浏览器登录超时，请重试。");
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
