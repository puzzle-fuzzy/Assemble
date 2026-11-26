import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import {
  getDisplayPreferences,
  setLanguage,
  setTheme,
} from "@assemble/display";
import { act } from "react";
import { App } from "./App";
import {
  button,
  cleanup,
  click,
  navigate,
  render,
  search,
  settle,
  waitForText,
} from "./test-support/dom";

const createdAt = "2026-11-25T01:02:03.000Z";
const account = {
  id: "owner",
  displayName: "管理员本人",
  email: "owner@example.test",
  avatarUrl: null,
};
const user = {
  id: "user",
  displayName: "张三",
  email: "zhang@example.test",
  avatarUrl: null,
  createdAt,
  lastLoginAt: null,
  hasGithub: true,
  isSuperAdmin: false,
  deviceCount: 2,
  onlineDeviceCount: 1,
};
const device = {
  id: "device",
  accountId: user.id,
  accountDisplayName: user.displayName,
  accountEmail: user.email,
  name: "工作电脑",
  platform: "other",
  online: false,
  createdAt,
  lastSeenAt: null,
};
const login = {
  id: "login",
  accountId: user.id,
  displayName: user.displayName,
  email: null,
  method: "email",
  occurredAt: createdAt,
};
const originalFetch = globalThis.fetch;
const requested: string[] = [];
let handle: (path: string) => Response | Promise<Response>;

function json(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function response(path: string): Response {
  if (path === "/v1/admin/session")
    return json({ account, csrfToken: "test-csrf" });
  if (path === "/v1/admin/overview")
    return json({
      users: 1234,
      devices: 2001,
      onlineDevices: 1,
      registrationsToday: 2,
    });
  if (path === "/v1/admin/users/user")
    return json({ user, devices: [device], logins: [login] });
  const url = new URL(path, "http://localhost");
  const items = url.pathname.endsWith("users")
    ? [user]
    : url.pathname.endsWith("devices")
      ? [device]
      : [login];
  return json({
    items,
    total: 1234,
    page: Number(url.searchParams.get("page") ?? "1"),
    pageSize: Number(url.searchParams.get("pageSize") ?? "20"),
  });
}

async function changeLanguage(language: "zh-CN" | "en") {
  await act(async () => setLanguage(language));
  await settle();
}

function menuItem(name: string, role = "menuitem") {
  const item = [
    ...document.body.querySelectorAll<HTMLElement>(`[role="${role}"]`),
  ].find((candidate) => candidate.textContent?.trim() === name);
  if (!item) throw new Error(`找不到菜单项：${name}`);
  return item;
}

async function choosePreference(
  host: HTMLElement,
  menu: string,
  category: string,
  choice: string,
) {
  const trigger = button(host, menu);
  await act(async () => {
    trigger.focus();
    trigger.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
    );
  });
  await settle();
  const categoryItem = menuItem(category);
  await act(async () => {
    categoryItem.focus();
    categoryItem.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }),
    );
  });
  await settle();
  await click(menuItem(choice, "menuitemradio"));
  // 语言或外观的单选项关闭时，将剩余的父菜单也交还给触发器。
  for (let level = 0; level < 2; level++) {
    await act(async () => {
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });
    await settle();
  }
}

beforeEach(() => {
  setLanguage("zh-CN");
  setTheme("system");
  document.body.replaceChildren();
  window.history.replaceState(null, "", "/admin/");
  requested.length = 0;
  handle = response;
  globalThis.fetch = mock((input: RequestInfo | URL) => {
    const path = String(input);
    requested.push(path);
    return Promise.resolve(handle(path));
  }) as unknown as typeof fetch;
});

afterEach(async () => {
  await cleanup();
  globalThis.fetch = originalFetch;
  setLanguage("zh-CN");
  setTheme("system");
  (
    window as unknown as {
      happyDOM: {
        setWindowSize: (size: { width: number; height: number }) => void;
      };
    }
  ).happyDOM.setWindowSize({ width: 1024, height: 768 });
});

describe("管理端中英文与显示偏好", () => {
  test("未登录也能通过真实菜单切换语言和外观，不读取管理数据", async () => {
    handle = () => json({}, 401);
    const host = await render(<App />);
    expect(host.textContent).toContain("登录管理控制台");
    await choosePreference(host, "语言与外观", "语言", "English");
    expect(host.textContent).toContain("Sign in to Assemble Admin");
    expect(button(host, "Language and appearance")).toBeDefined();
    expect(document.title).toBe("Assemble Admin");
    expect(document.documentElement.lang).toBe("en");
    expect(host.querySelector("a")?.getAttribute("href")).toBe(
      "/web/login?next=/admin/",
    );
    await choosePreference(
      host,
      "Language and appearance",
      "Appearance",
      "Dark",
    );
    expect(getDisplayPreferences().theme).toBe("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    await choosePreference(
      host,
      "Language and appearance",
      "Appearance",
      "System",
    );
    expect(getDisplayPreferences().theme).toBe("system");
    expect(requested).toEqual(["/v1/admin/session"]);
    await choosePreference(
      host,
      "Language and appearance",
      "Language",
      "简体中文",
    );
    expect(host.textContent).toContain("登录管理控制台");
  });

  test.each([401, 403])("英文权限状态 %s 保留原有安全入口", async (status) => {
    setLanguage("en");
    handle = () => json({ detail: "sensitive response" }, status);
    const host = await render(<App />);
    expect(host.textContent).toContain(
      status === 401 ? "Sign in to Assemble Admin" : "Admin access required",
    );
    expect(host.querySelector("a")?.getAttribute("href")).toBe(
      status === 401 ? "/web/login?next=/admin/" : "/web/account",
    );
    expect(host.textContent).not.toContain("sensitive response");
    expect(host.querySelector('input[type="password"]')).toBeNull();
    expect(button(host, "Language and appearance")).toBeDefined();
  });

  test("切换语言保留搜索焦点、用户数据和分页，并翻译详情及北京时间格式", async () => {
    const host = await render(<App />);
    expect(host.textContent).toContain("1,234");
    await navigate(host, "users");
    await search(host, "张三");
    const input = host.querySelector<HTMLInputElement>('input[type="search"]');
    input?.focus();
    const before = requested.length;
    await changeLanguage("en");
    expect(requested).toHaveLength(before);
    expect(document.activeElement).toBe(input);
    expect(input?.value).toBe("张三");
    expect(input?.placeholder).toBe("Name or email");
    expect(
      host.querySelector('[data-slot="breadcrumb-page"]')?.textContent,
    ).toBe("Users");
    expect(document.title).toBe("Users · Assemble Admin");
    expect(host.textContent).toContain("张三");
    expect(host.textContent).toContain("zhang@example.test");
    expect(host.textContent).toContain("Not recorded");
    expect(host.textContent).toContain("2 devices / 1 online");
    expect(host.querySelector('thead [role="checkbox"]')).not.toBeNull();
    expect(host.querySelector('th[data-table-column="drag"]')).not.toBeNull();
    expect(host.textContent).toContain("Page 1 / 124");
    expect(host.querySelector("time")?.getAttribute("datetime")).toBe(
      createdAt,
    );
    expect(host.querySelector("time")?.textContent).toContain("09:02:03");
    expect(host.querySelector("time")?.title).toBe("Beijing time (UTC+8)");
    await click(button(host, "Next page"));
    const parameters = new URL(requested.at(-1) ?? "", "http://localhost")
      .searchParams;
    expect(parameters.get("page")).toBe("2");
    expect(parameters.get("q")).toBe("张三");
    await click(button(host, "View details for 张三"));
    expect(
      host.querySelector('[data-slot="breadcrumb-page"]')?.textContent,
    ).toBe("Users");
    await waitForText(document.body, "Account information");
    const detail = document.body.querySelector<HTMLElement>('[role="dialog"]');
    expect(detail?.textContent).toContain("2 devices");
    expect(detail?.textContent).toContain("1 device");
    expect(detail?.textContent).toContain("工作电脑");
    expect(detail?.textContent).toContain("Email and password");
    const detailRequests = requested.length;
    await changeLanguage("zh-CN");
    expect(
      host.querySelector('[data-slot="breadcrumb-page"]')?.textContent,
    ).toBe("用户");
    expect(detail?.textContent).toContain("账户信息");
    expect(detail?.textContent).toContain("2 台");
    expect(requested).toHaveLength(detailRequests);
    await changeLanguage("en");
    expect(document.title).toBe("Users · Assemble Admin");
    await click(button(document.body, "Close"));
    expect(
      host.querySelector<HTMLInputElement>('input[type="search"]')?.value,
    ).toBe("张三");
    expect(host.textContent).toContain("Page 2 / 124");
    await changeLanguage("zh-CN");
    expect(host.textContent).toContain("2 台 / 1 在线");
    expect(host.textContent).toContain("第 2 / 124 页");
    expect(document.title).toBe("用户 · Assemble 管理控制台");
  });

  test("窄屏登录后仍可从功能菜单切换语言和外观", async () => {
    (
      window as unknown as {
        happyDOM: {
          setWindowSize: (size: { width: number; height: number }) => void;
        };
      }
    ).happyDOM.setWindowSize({ width: 720, height: 480 });
    const host = await render(<App />);
    await choosePreference(host, "打开管理菜单", "语言", "English");
    expect(document.title).toBe("Overview · Assemble Admin");
    await choosePreference(host, "Open admin menu", "Appearance", "Dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(
      host.querySelector('[data-slot="breadcrumb-page"]')?.textContent,
    ).toBe("Overview");
    expect(host.textContent).toContain("1,234");
  });

  test.each(["devices", "logins"])(
    "英文 %s 表格的列标题、可访问名称和搜索分页同步切换",
    async (view) => {
      setLanguage("en");
      const host = await render(<App />);
      await navigate(host, view);
      expect(
        host.querySelector('nav[aria-label="Administration"]'),
      ).not.toBeNull();
      expect(
        host.querySelector('nav[aria-label="Table pagination"]'),
      ).not.toBeNull();
      expect(
        host
          .querySelector<HTMLElement>("section[tabindex='0']")
          ?.getAttribute("aria-label"),
      ).toContain("scroll horizontally");
      expect(host.textContent).toContain(
        view === "devices" ? "Device name" : "Sign-in method",
      );
      expect(host.textContent).toContain(
        view === "devices" ? "Other" : "Email and password",
      );
      if (view === "devices") {
        expect(host.textContent).toContain("Offline");
        expect(host.textContent).toContain("工作电脑");
      } else expect(host.textContent).toContain("No linked email");
      await click(button(host, "Next page"));
      await search(host, "张三");
      const parameters = new URL(requested.at(-1) ?? "", "http://localhost")
        .searchParams;
      expect(parameters.get("q")).toBe("张三");
      expect(parameters.get("page")).toBe("1");
    },
  );

  test("英文加载、错误重试和搜索空态均显示翻译后的恢复操作", async () => {
    setLanguage("en");
    let resolveOverview: ((value: Response) => void) | undefined;
    let attempts = 0;
    handle = (path) => {
      if (path === "/v1/admin/overview" && attempts++ === 0) {
        return new Promise((resolve) => {
          resolveOverview = resolve;
        });
      }
      if (path.startsWith("/v1/admin/users?"))
        return json({ items: [], total: 0, page: 1, pageSize: 20 });
      return response(path);
    };
    const host = await render(<App />);
    expect(host.textContent).toContain("Loading");
    await act(async () => resolveOverview?.(json({ detail: "private" }, 503)));
    await settle();
    expect(host.textContent).toContain("Unable to load data");
    expect(host.textContent).not.toContain("private");
    await click(button(host, "Retry"));
    expect(host.textContent).toContain("Registered users");
    await navigate(host, "users");
    expect(host.textContent).toContain("No registered users yet");
    expect(host.textContent).toContain("Page 1 / 1");
    await search(host, "nobody");
    expect(host.textContent).toContain("No matching results");
    expect(button(host, "Next page").disabled).toBe(true);
    await changeLanguage("zh-CN");
    expect(host.textContent).toContain("没有匹配的结果");
    expect(button(host, "下一页").disabled).toBe(true);
  });
});
