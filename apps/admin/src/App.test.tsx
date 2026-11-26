import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import { App } from "./App";
import type {
  AdminDevice,
  AdminLogin,
  AdminPage,
  AdminSession,
  AdminUser,
} from "./lib/api";
import {
  button,
  cleanup,
  click,
  navigate,
  render,
  search,
  settle,
  waitFor,
  waitForText,
} from "./test-support/dom";

const session: AdminSession = {
  account: {
    id: "admin-1",
    displayName: "测试管理账户",
    email: "admin@example.test",
    avatarUrl: null,
  },
  csrfToken: "test-csrf-only",
};
const user: AdminUser = {
  id: "user-1",
  displayName: "张三",
  email: "zhang@example.test",
  avatarUrl: null,
  createdAt: "2026-11-25T01:02:03.000Z",
  lastLoginAt: null,
  hasGithub: true,
  isSuperAdmin: false,
  deviceCount: 2,
  onlineDeviceCount: 1,
};
const device: AdminDevice = {
  id: "device-1",
  accountId: user.id,
  accountDisplayName: user.displayName,
  accountEmail: user.email,
  name: "工作电脑",
  platform: "windows",
  online: true,
  createdAt: user.createdAt,
  lastSeenAt: null,
};
const login: AdminLogin = {
  id: "login-1",
  accountId: user.id,
  displayName: user.displayName,
  email: user.email,
  method: "github",
  occurredAt: user.createdAt,
};

const originalFetch = globalThis.fetch;
const requests: { path: string; init: RequestInit | undefined }[] = [];
let handle: (path: string, init?: RequestInit) => Response | Promise<Response>;

function json(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function page<T>(items: T[], options: Partial<AdminPage<T>> = {}) {
  return { items, total: items.length, page: 1, pageSize: 20, ...options };
}

function defaultResponse(path: string): Response {
  if (path === "/v1/admin/session") return json(session);
  if (path === "/v1/admin/overview")
    return json({
      users: 23,
      devices: 41,
      onlineDevices: 8,
      registrationsToday: 3,
    });
  if (path.startsWith("/v1/admin/users?")) return json(page([user]));
  if (path === `/v1/admin/users/${user.id}`)
    return json({ user, devices: [device], logins: [login] });
  if (path.startsWith("/v1/admin/devices?")) return json(page([device]));
  if (path.startsWith("/v1/admin/logins?")) return json(page([login]));
  return json({}, 404);
}

beforeEach(() => {
  setLanguage("zh-CN");
  document.body.replaceChildren();
  window.history.replaceState(null, "", "/admin/");
  requests.length = 0;
  handle = defaultResponse;
  globalThis.fetch = mock((input: RequestInfo | URL, init?: RequestInit) => {
    const path = String(input);
    requests.push({ path, init });
    return Promise.resolve(handle(path, init));
  }) as unknown as typeof fetch;
});

afterEach(async () => {
  await cleanup();
  globalThis.fetch = originalFetch;
});

describe("管理端真实数据和访问权限", () => {
  test("未登录通过当前网页进入管理登录，不请求管理数据", async () => {
    handle = () => json({ message: "sensitive body" }, 401);
    const host = await render(<App />);
    expect(host.textContent).toContain("登录管理控制台");
    expect(host.querySelector("a")?.getAttribute("href")).toBe(
      "/web/login?next=/admin/",
    );
    expect(host.querySelector('input[type="password"]')).toBeNull();
    expect(requests.map((request) => request.path)).toEqual([
      "/v1/admin/session",
    ]);
    expect(host.textContent).not.toContain("sensitive body");
  });

  test("普通账户显示无权限，首位注册者也不会在前端被授权", async () => {
    handle = () => json({}, 403);
    const host = await render(<App />);
    expect(host.textContent).toContain("没有管理权限");
    expect(host.querySelector('nav[aria-label="管理功能"]')).toBeNull();
    expect(host.querySelector("a")?.getAttribute("href")).toBe("/web/account");
    expect(host.textContent).toContain("请切换到超级管理员账户");
  });

  test("会话失败显示通用错误，点击重试重新核验身份", async () => {
    let failed = true;
    handle = (path) =>
      path === "/v1/admin/session" && failed
        ? json({ token: "do-not-display" }, 500)
        : defaultResponse(path);
    const host = await render(<App />);
    expect(host.textContent).toContain("暂时无法读取数据");
    expect(host.textContent).not.toContain("do-not-display");
    failed = false;
    await click(button(host, "重试"));
    expect(
      host.querySelector('header [data-slot="breadcrumb-page"]')?.textContent,
    ).toBe("概览");
  });

  test("加载只显示 Skeleton，拿到结果后展示实际数量，菜单导航对应右侧内容", async () => {
    let resolveOverview: ((value: Response) => void) | undefined;
    handle = (path) =>
      path === "/v1/admin/overview"
        ? new Promise((resolve) => {
            resolveOverview = resolve;
          })
        : defaultResponse(path);
    const host = await render(<App />);
    expect(host.querySelector('[data-slot="skeleton"]')).not.toBeNull();
    expect(host.textContent).not.toContain("23");
    await act(async () =>
      resolveOverview?.(defaultResponse("/v1/admin/overview")),
    );
    expect(host.textContent).toContain("23");
    expect(host.querySelector("main h1") === null).toBe(true);
    expect(host.querySelector('[data-slot="skeleton"]')).toBeNull();
    await navigate(host, "devices");
    expect(
      host.querySelector('header [data-slot="breadcrumb-page"]')?.textContent,
    ).toBe("设备");
    expect(host.textContent).toContain("工作电脑");
    expect(host.textContent).toContain("Windows");
    expect(document.activeElement).toBe(
      host.querySelector('header [data-slot="breadcrumb-page"]'),
    );
    expect(
      host.querySelectorAll('nav[aria-label="管理功能"] [aria-current="page"]'),
    ).toHaveLength(1);
    expect(
      host.querySelector('[aria-current="page"][href="#devices"]'),
    ).not.toBeNull();
    await navigate(host, "logins");
    expect(
      host.querySelector('header [data-slot="breadcrumb-page"]')?.textContent,
    ).toBe("登录记录");
    expect(host.textContent).toContain("GitHub");
    expect(document.title).toBe("登录记录 · Assemble 管理控制台");
  });

  test("所有数据请求依赖同源 Cookie，网页退出后返回管理登录入口", async () => {
    const host = await render(<App />);
    for (const { init } of requests) {
      expect(init?.credentials).toBe("same-origin");
      expect(init?.cache).toBe("no-store");
      expect(init?.redirect).toBe("error");
      expect(init?.headers).toEqual({ Accept: "application/json" });
      expect(init?.signal).toBeInstanceOf(AbortSignal);
    }
    expect(
      host
        .querySelector("form[action='/web/logout?next=/admin/']")
        ?.getAttribute("method"),
    ).toBe("post");
    expect(
      host.querySelector<HTMLInputElement>('input[name="csrf"]')?.value,
    ).toBe("test-csrf-only");
    expect(host.textContent).not.toContain("test-csrf-only");
  });

  test("管理数据请求失去权限时立即移除旧账户数据", async () => {
    const host = await render(<App />);
    await navigate(host, "users");
    await waitForText(host, "zhang@example.test");
    handle = (path) =>
      path.startsWith("/v1/admin/devices")
        ? json({}, 401)
        : defaultResponse(path);
    await navigate(host, "devices");
    await waitForText(host, "登录管理控制台");
    expect(host.textContent).not.toContain(user.email);
    expect(host.textContent).not.toContain(session.account.displayName);
  });

  test("管理员头像菜单位于侧栏底部，键盘退出提交原有同源表单", async () => {
    const host = await render(<App />);
    expect(host.querySelector('header [data-slot="avatar"]')).toBeNull();
    const trigger = button(host, "管理员账户菜单");
    expect(trigger.closest('[data-slot="sidebar-footer"]')).not.toBeNull();
    expect(trigger.textContent).toContain(session.account.displayName);
    expect(trigger.textContent).toContain("admin@example.test");
    const form = host.querySelector<HTMLFormElement>(
      "form[action='/web/logout?next=/admin/']",
    );
    if (!form) throw new Error("缺少退出表单");
    const submit = mock(() => {});
    form.requestSubmit = submit;
    await act(async () => {
      trigger.focus();
      trigger.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
      );
    });
    await settle();
    const item = [
      ...document.body.querySelectorAll<HTMLElement>('[role="menuitem"]'),
    ].find((candidate) => candidate.textContent === "退出登录");
    expect(item?.textContent).toBe("退出登录");
    if (!item) throw new Error("缺少退出菜单项");
    await click(item);
    expect(submit).toHaveBeenCalledTimes(1);
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });
});

describe("用户、设备和登录记录的交互", () => {
  test("用户列表搜索会编码关键词并回第一页，详情显示设备和登录记录，返回保留查询", async () => {
    handle = (path) => {
      if (path.startsWith("/v1/admin/users?")) {
        const url = new URL(path, "http://localhost");
        return json(
          page([user], {
            total: 45,
            page: Number(url.searchParams.get("page")),
          }),
        );
      }
      return defaultResponse(path);
    };
    const host = await render(<App />);
    await navigate(host, "users");
    await waitForText(host, "未记录");
    expect(host.textContent).toContain("2 台 / 1 在线");
    expect(host.querySelector("time")?.getAttribute("datetime")).toBe(
      user.createdAt,
    );
    await click(button(host, "下一页"));
    expect(requests.at(-1)?.path).toContain("page=2");
    await search(host, " 张三+工作@example.test ");
    const query = new URL(requests.at(-1)?.path ?? "", "http://localhost")
      .searchParams;
    expect(query.get("page")).toBe("1");
    expect(query.get("q")).toBe("张三+工作@example.test");
    await click(button(host, "查看 张三 的详情"));
    expect(
      host.querySelector('header [data-slot="breadcrumb-page"]')?.textContent,
    ).toBe("用户详情");
    expect(host.textContent).toContain("工作电脑");
    expect(host.textContent).toContain("最近登录记录");
    expect(host.textContent).toContain("GitHub");
    expect(document.activeElement).toBe(
      host.querySelector('header [data-slot="breadcrumb-page"]'),
    );
    await click(button(host, "返回用户列表"));
    expect(
      host.querySelector<HTMLInputElement>('input[type="search"]')?.value,
    ).toBe(" 张三+工作@example.test ");
    expect(
      host.querySelector('header [data-slot="breadcrumb-page"]')?.textContent,
    ).toBe("用户");
    expect(document.activeElement).toBe(
      host.querySelector('header [data-slot="breadcrumb-page"]'),
    );
  });

  test("列表读取失败后重试，空结果使用真正空态，空邮箱与时间不编造", async () => {
    let failed = true;
    handle = (path) =>
      path.startsWith("/v1/admin/users?")
        ? failed
          ? json({ detail: "private" }, 503)
          : json(page([{ ...user, email: null, lastLoginAt: null }]))
        : defaultResponse(path);
    const host = await render(<App />);
    await navigate(host, "users");
    await waitFor(
      () => host.querySelector('[role="alert"]') !== null,
      "出错提示",
    );
    expect(host.textContent).not.toContain("private");
    failed = false;
    await click(button(host, "重试"));
    expect(host.textContent).toContain("未绑定邮箱");
    expect(host.textContent).toContain("未记录");
    handle = (path) =>
      path.startsWith("/v1/admin/users?")
        ? json(page([]))
        : defaultResponse(path);
    await search(host, "没有这个用户");
    expect(host.querySelector('[data-slot="empty"]')).not.toBeNull();
    expect(host.textContent).toContain("没有匹配的结果");
    expect(button(host, "下一页").disabled).toBe(true);
  });

  test.each(["devices", "logins"])(
    "%s 使用服务端搜索与分页，并保留空值",
    async (view) => {
      handle = (path) => {
        if (path.startsWith(`/v1/admin/${view}?`)) {
          const parameters = new URL(path, "http://localhost").searchParams;
          const items: (AdminDevice | AdminLogin)[] =
            view === "devices"
              ? [
                  {
                    ...device,
                    accountEmail: null,
                    online: false,
                    platform: "other",
                  },
                ]
              : [{ ...login, email: null, method: "email" }];
          return json(
            page(items, { total: 21, page: Number(parameters.get("page")) }),
          );
        }
        return defaultResponse(path);
      };
      const host = await render(<App />);
      await navigate(host, view);
      await waitForText(host, "未绑定邮箱");
      expect(host.textContent).toContain(
        view === "devices" ? "离线" : "邮箱密码",
      );
      await click(button(host, "下一页"));
      expect(requests.at(-1)?.path).toContain("page=2");
      await search(host, "张三");
      const parameters = new URL(
        requests.at(-1)?.path ?? "",
        "http://localhost",
      ).searchParams;
      expect(parameters.get("q")).toBe("张三");
      expect(parameters.get("page")).toBe("1");
    },
  );

  test("过期搜索即使后返回也不会覆盖新结果，导航离开会取消请求", async () => {
    let oldResolve: ((value: Response) => void) | undefined;
    let oldSignal: AbortSignal | null | undefined;
    handle = (path, init) => {
      if (
        path.startsWith("/v1/admin/users?") &&
        new URL(path, "http://localhost").searchParams.get("q") === "旧查询"
      ) {
        oldSignal = init?.signal;
        return new Promise((resolve) => {
          oldResolve = resolve;
        });
      }
      if (
        path.startsWith("/v1/admin/users?") &&
        new URL(path, "http://localhost").searchParams.get("q") === "新查询"
      ) {
        return json(page([{ ...user, displayName: "新结果" }]));
      }
      return defaultResponse(path);
    };
    const host = await render(<App />);
    await navigate(host, "users");
    await search(host, "旧查询");
    expect(host.querySelector('[data-slot="skeleton"]')).not.toBeNull();
    expect(host.textContent).not.toContain(user.email);
    await search(host, "新查询");
    expect(oldSignal?.aborted).toBe(true);
    await act(async () =>
      oldResolve?.(json(page([{ ...user, displayName: "旧结果" }]))),
    );
    expect(host.textContent).toContain("新结果");
    expect(host.textContent).not.toContain("旧结果");
    await search(host, "旧查询");
    await navigate(host, "devices");
    expect(oldSignal?.aborted).toBe(true);
    await act(async () => oldResolve?.(json(page([user]))));
    await settle();
    expect(
      host.querySelector('header [data-slot="breadcrumb-page"]')?.textContent,
    ).toBe("设备");
  });

  test("用户详情 404 可以返回，缺少历史时显示空态而不是样例内容", async () => {
    let missing = true;
    handle = (path) =>
      path === `/v1/admin/users/${user.id}`
        ? missing
          ? json({}, 404)
          : json({ user, devices: [], logins: [] })
        : defaultResponse(path);
    const host = await render(<App />);
    await navigate(host, "users");
    await click(button(host, "查看 张三 的详情"));
    await waitForText(host, "找不到这位用户");
    await click(button(host, "返回用户列表"));
    missing = false;
    await click(button(host, "查看 张三 的详情"));
    expect(host.textContent).toContain("这个账户还没有设备");
    expect(host.textContent).toContain("这个账户还没有登录记录");
    expect(host.textContent).not.toContain("工作电脑");
  });
});
