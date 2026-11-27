import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import samples from "../contract/dht.json";
import { DhtPage } from "./components/dht-page";
import type { AdminSession } from "./lib/api";
import type { DhtDetails, DhtStatus } from "./lib/dht-api";
import en from "./locales/dht.en.json";
import zh from "./locales/dht.zh-CN.json";
import {
  adminLocation,
  button,
  cleanup,
  click,
  render,
  search,
  settle,
  waitFor,
  waitForText,
} from "./test-support/dom";

const session: AdminSession = {
  account: {
    id: "admin",
    displayName: "管理员",
    email: "admin@example.test",
    avatarUrl: null,
  },
  csrfToken: "synthetic-csrf",
};
const originalFetch = globalThis.fetch;
let restoreLocation: () => void;
const requests: { path: string; init?: RequestInit }[] = [];
let handle: (path: string, init?: RequestInit) => Response | Promise<Response>;
function json(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}
function normal(path: string) {
  if (path.endsWith("/status")) return json(samples.status);
  if (path.includes("/observations?"))
    return json({
      items: [samples.detail],
      total: 1,
      page: 1,
      pageSize: Number(
        new URL(path, "http://localhost").searchParams.get("pageSize") ?? 20,
      ),
    });
  if (path.includes("/observations/")) return json(samples.detail);
  return json({}, 404);
}

async function chooseMode(host: HTMLElement, current: string, action: string) {
  await act(async () => {
    const trigger = button(host, current);
    trigger.focus();
    trigger.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
    );
  });
  await waitFor(
    () => Boolean(document.querySelector('[role="menuitem"]')),
    "采集操作菜单",
  );
  const option = [
    ...document.querySelectorAll<HTMLElement>('[role="menuitem"]'),
  ].find((item) => item.textContent?.trim() === action);
  if (!option) throw new Error(`找不到采集操作：${action}`);
  await click(option);
}
beforeEach(async () => {
  restoreLocation = adminLocation("/admin/#dht");
  requests.length = 0;
  handle = normal;
  await setLanguage("zh-CN");
  globalThis.fetch = mock(
    async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      requests.push({ path, init });
      return handle(path, init);
    },
  ) as unknown as typeof fetch;
});
afterEach(async () => {
  await cleanup();
  restoreLocation();
  globalThis.fetch = originalFetch;
});

test("Rust and browser samples share metadata and complete translation keys", () => {
  const detail: DhtDetails = { ...samples.detail, status: "verified" };
  const status: DhtStatus = { ...samples.status, mode: "collecting" };
  expect(detail.files[0]?.length).toBe(3);
  expect(status.metadataBudgetBytes).toBe(536870912);
  const keys = (value: object): string[] =>
    Object.entries(value)
      .flatMap(([key, child]) =>
        typeof child === "object" && child !== null
          ? keys(child).map((nested) => `${key}.${nested}`)
          : [key],
      )
      .sort();
  expect(keys(en)).toEqual(keys(zh));
});

test("uses shared tables, server search, details and English translations", async () => {
  const host = await render(
    <DhtPage session={session} onAccessFailure={() => {}} />,
  );
  await waitForText(host, "sample.txt");
  expect(host.querySelector("table")).not.toBeNull();
  await click(button(host, "sample.txt"));
  await waitFor(
    () => Boolean(document.querySelector('[role="dialog"]')),
    "详情",
  );
  const dialog = document.querySelector('[role="dialog"]') as HTMLElement;
  await waitForText(dialog, "文件清单");
  expect(dialog.textContent).toContain(samples.detail.id);
  await click(button(dialog, "关闭"));
  await search(host, "another");
  expect(requests.some(({ path }) => path.includes("q=another"))).toBe(true);
  await act(async () => {
    await setLanguage("en");
  });
  expect(host.textContent).toContain("Collecting");
  expect(host.textContent).not.toContain("statuses.");
});

test("control writes use CSRF without putting it in a URL", async () => {
  handle = (path, init) =>
    path.endsWith("/control")
      ? json(JSON.parse(String(init?.body)))
      : normal(path);
  const host = await render(
    <DhtPage session={session} onAccessFailure={() => {}} />,
  );
  await waitForText(host, "sample.txt");
  await chooseMode(host, "采集中", "暂停采集");
  const written = requests.find(({ path }) => path.endsWith("/control"));
  expect(written?.init?.method).toBe("POST");
  expect(written?.init?.credentials).toBe("same-origin");
  expect(new Headers(written?.init?.headers).get("X-CSRF-Token")).toBe(
    session.csrfToken,
  );
  expect(JSON.parse(String(written?.init?.body))).toEqual({ mode: "draining" });
  expect(requests.every(({ path }) => !path.includes(session.csrfToken))).toBe(
    true,
  );
});

test("only explicit actions refresh the list, runtime and pending metadata", async () => {
  const originalTimeout = globalThis.setTimeout;
  // 加速经过的时间，让意外保留的周期请求暴露出来，而不用让测试等待半分钟。
  globalThis.setTimeout = ((...arguments_: Parameters<typeof setTimeout>) => {
    arguments_[1] = Math.min(Number(arguments_[1]) || 0, 1);
    return originalTimeout(...arguments_);
  }) as typeof setTimeout;
  handle = (path, init) => {
    if (path.endsWith("/control")) return json(JSON.parse(String(init?.body)));
    if (path.includes("/observations/"))
      return json({ ...samples.detail, status: "fetching" });
    return normal(path);
  };
  const reads = () =>
    requests.filter(({ init }) => !init?.method || init.method === "GET")
      .length;
  async function idle() {
    for (let step = 0; step < 8; step++) await settle();
  }
  try {
    const host = await render(
      <DhtPage session={session} onAccessFailure={() => {}} />,
    );
    await waitForText(host, "sample.txt");
    const initial = reads();
    await idle();
    expect(reads()).toBe(initial);

    await click(button(host, "sample.txt"));
    const dialog = document.querySelector('[role="dialog"]') as HTMLElement;
    await waitForText(dialog, "解析中");
    const withDetail = reads();
    await idle();
    expect(reads()).toBe(withDetail);
    await click(button(dialog, "刷新"));
    await waitFor(() => reads() === withDetail + 1, "手动刷新详情");
    await click(button(dialog, "关闭"));

    await chooseMode(host, "采集中", "暂停采集");
    await waitForText(host, "正在排空队列");
    const afterControl = reads();
    await idle();
    expect(reads()).toBe(afterControl);
    await click(button(host, "刷新"));
    await waitFor(() => reads() === afterControl + 2, "手动刷新列表和状态");
  } finally {
    await cleanup();
    globalThis.setTimeout = originalTimeout;
  }
});

test("runtime diagnostics stay behind a named icon and status buttons open details", async () => {
  const host = await render(
    <DhtPage session={session} onAccessFailure={() => {}} />,
  );
  await waitForText(host, "sample.txt");
  expect(host.textContent).not.toContain("每 30 秒刷新");
  expect(host.textContent).not.toContain("全网热度");
  expect(host.textContent).not.toContain("记录上限");
  expect(host.querySelector("details")).toBeNull();
  expect(host.querySelector('[data-slot="badge"]')).toBeNull();
  await click(button(host, "运行状态"));
  const runtime = document.querySelector('[role="dialog"]') as HTMLElement;
  await waitForText(runtime, "记录上限");
  expect(runtime.textContent).toContain("文件清单存储");
  await click(button(runtime, "关闭"));
  await click(button(host, "已验证"));
  await waitForText(
    document.querySelector('[role="dialog"]') as HTMLElement,
    "文件清单",
  );
});

test("invalid queries show a fixed error and closing details aborts the request", async () => {
  handle = (path) =>
    path.endsWith("/query")
      ? json(
          { error: { code: "INVALID_REQUEST", message: "untrusted secret" } },
          400,
        )
      : normal(path);
  const host = await render(
    <DhtPage session={session} onAccessFailure={() => {}} />,
  );
  await waitForText(host, "sample.txt");
  await click(button(host, "查询哈希"));
  const dialog = document.querySelector('[role="dialog"]') as HTMLElement;
  const input = dialog.querySelector("input") as HTMLInputElement;
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set?.call(input, "bad-hash");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await click(button(dialog, "查询元数据"));
  await waitForText(dialog, "请输入有效的 Info-hash");
  expect(dialog.textContent).not.toContain("untrusted secret");
  await click(button(dialog, "关闭"));
  handle = (path, init) =>
    path.includes("/observations/")
      ? new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener("abort", () =>
            reject(new DOMException("Aborted", "AbortError")),
          );
        })
      : normal(path);
  await click(button(host, "sample.txt"));
  await waitFor(
    () => requests.some(({ path }) => path.includes("/observations/")),
    "详情请求",
  );
  await click(
    button(document.querySelector('[role="dialog"]') as HTMLElement, "关闭"),
  );
  expect(
    [...requests].reverse().find(({ path }) => path.includes("/observations/"))
      ?.init?.signal?.aborted,
  ).toBe(true);
});

test("revoked administrator access returns to the shell", async () => {
  const deny = mock(() => {});
  handle = () => json({}, 403);
  await render(<DhtPage session={session} onAccessFailure={deny} />);
  await waitFor(() => deny.mock.calls.length > 0, "权限失效");
  expect(deny).toHaveBeenCalledWith("forbidden");
});

test("copy confirmation stays visible inside the metadata dialog", async () => {
  const original = Object.getOwnPropertyDescriptor(navigator, "clipboard");
  const copy = mock(async () => {});
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: copy },
  });
  try {
    const host = await render(
      <DhtPage session={session} onAccessFailure={() => {}} />,
    );
    await waitForText(host, "sample.txt");
    await click(button(host, "sample.txt"));
    const dialog = document.querySelector('[role="dialog"]') as HTMLElement;
    await waitForText(dialog, "文件清单");
    await click(button(dialog, "复制磁力链接"));
    await waitForText(dialog, "已复制磁力链接");
    expect(copy).toHaveBeenCalledWith(samples.detail.magnetUri);
  } finally {
    if (original) Object.defineProperty(navigator, "clipboard", original);
    else Reflect.deleteProperty(navigator, "clipboard");
  }
});
