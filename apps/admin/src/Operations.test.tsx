import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import { App } from "./App";
import { AccountSuspension } from "./components/account-suspension";
import { StorageOverview } from "./components/storage-overview";
import type { AdminSession, AdminStorage, AdminUser } from "./lib/api";
import {
  button,
  cleanup,
  click,
  navigate,
  render,
  settle,
  waitFor,
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
const initialUser: AdminUser = {
  id: "member",
  displayName: "成员<测试>",
  email: "member@example.test",
  avatarUrl: null,
  createdAt: "2026-11-25T01:02:03.000Z",
  lastLoginAt: null,
  hasGithub: false,
  isSuperAdmin: false,
  suspended: false,
  deviceCount: 0,
  onlineDeviceCount: 0,
};
const storage: AdminStorage = {
  contentBytes: 800,
  budgetBytes: 1000,
  databaseBytes: 4096,
  diskAvailableBytes: 1024,
  warningPercent: 80,
  budgetWarning: true,
  diskWarning: true,
  topAccounts: [{ ...initialUser, contentBytes: 700 }],
};
const savedFetch = globalThis.fetch;
let user: AdminUser;
let calls: { path: string; init?: RequestInit }[];
let handleWrite: (
  path: string,
  init?: RequestInit,
) => Response | Promise<Response>;
function json(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" },
  });
}
beforeEach(() => {
  setLanguage("zh-CN");
  window.history.replaceState(null, "", "/admin/");
  user = { ...initialUser };
  calls = [];
  handleWrite = (path) => {
    user = { ...user, suspended: path.endsWith("/suspend") };
    return json(user);
  };
  globalThis.fetch = mock((input: RequestInfo | URL, init?: RequestInit) => {
    const path = String(input);
    calls.push({ path, init });
    if (init?.method === "POST")
      return Promise.resolve(handleWrite(path, init));
    if (path === "/v1/admin/session") return Promise.resolve(json(session));
    if (path === "/v1/admin/overview")
      return Promise.resolve(
        json({
          users: 1,
          devices: 0,
          onlineDevices: 0,
          registrationsToday: 1,
          storage,
        }),
      );
    if (path.startsWith("/v1/admin/users?"))
      return Promise.resolve(
        json({ items: [user], total: 1, page: 1, pageSize: 10 }),
      );
    if (path === `/v1/admin/users/${user.id}`)
      return Promise.resolve(json({ user, devices: [], logins: [] }));
    return Promise.resolve(json({}, 404));
  }) as unknown as typeof fetch;
});
afterEach(async () => {
  await cleanup();
  globalThis.fetch = savedFetch;
  await act(async () => setLanguage("zh-CN"));
});
async function details() {
  const host = await render(<App />);
  await navigate(host, "users");
  await click(button(host, `查看 ${user.displayName} 的详情`));
  await waitFor(
    () => document.body.textContent?.includes("账户信息") === true,
    "account details",
  );
  return host;
}
function alertDialog() {
  const dialog = document.body.querySelector<HTMLElement>(
    '[role="alertdialog"]',
  );
  if (!dialog) throw new Error("Missing confirmation dialog");
  return dialog;
}

test("停用与恢复都先确认，取消不写入，成功后同时更新详情和名单", async () => {
  const host = await details();
  expect(calls.filter((call) => call.init?.method === "POST")).toHaveLength(0);
  await click(button(document.body, "停用账户"));
  expect(alertDialog().textContent).toContain("成员<测试>");
  expect(alertDialog().textContent).toContain("所有设备会断开连接");
  expect(document.activeElement?.textContent).toBe("取消");
  await click(button(alertDialog(), "取消"));
  expect(calls.filter((call) => call.init?.method === "POST")).toHaveLength(0);
  await click(button(document.body, "停用账户"));
  await click(button(alertDialog(), "停用账户"));
  await waitFor(
    () => document.querySelector('[role="alertdialog"]') === null,
    "confirmation closed",
  );
  const posted = calls.filter((call) => call.init?.method === "POST");
  expect(posted).toHaveLength(1);
  expect(posted[0]?.path).toBe("/v1/admin/users/member/suspend");
  expect(new Headers(posted[0]?.init?.headers).get("x-csrf-token")).toBe(
    session.csrfToken,
  );
  expect(posted[0]?.init?.credentials).toBe("same-origin");
  expect(posted[0]?.init?.redirect).toBe("error");
  expect(host.querySelector("tbody")?.textContent).toContain("已停用");
  expect(button(document.body, "恢复账户")).toBeDefined();
  await click(button(document.body, "恢复账户"));
  await click(button(alertDialog(), "恢复账户"));
  expect(host.querySelector("tbody")?.textContent).toContain("正常");
  expect(
    calls.filter((call) => call.init?.method === "POST").at(-1)?.path,
  ).toBe("/v1/admin/users/member/resume");
});

test("CSRF 拒绝保留确认框和账户，错误诊断不进入界面，切换语言不重新提交", async () => {
  handleWrite = () =>
    json(
      { error: { code: "CSRF_REJECTED", message: "private Rust diagnostic" } },
      403,
    );
  await details();
  await click(button(document.body, "停用账户"));
  await click(button(alertDialog(), "停用账户"));
  expect(alertDialog().textContent).toContain("页面校验已失效");
  expect(document.body.textContent).not.toContain("private Rust diagnostic");
  const count = calls.length;
  await act(async () => setLanguage("en"));
  expect(calls).toHaveLength(count);
  expect(alertDialog().textContent).toContain("Page verification has expired");
  expect(alertDialog().textContent).toContain("成员<测试>");
  expect(button(alertDialog(), "Cancel")).toBeDefined();
});

test("不提供停用本人入口，迟到的操作响应不能更新已卸载账户", async () => {
  let finish: ((response: Response) => void) | undefined;
  const updated: AdminUser[] = [];
  handleWrite = () =>
    new Promise((resolve) => {
      finish = resolve;
    });
  const host = await render(
    <AccountSuspension
      user={user}
      session={{ ...session, account: user }}
      onUpdated={(value) => updated.push(value)}
      onAccessFailure={() => {}}
    />,
  );
  expect(host.querySelector("button")).toBeNull();
  await cleanup();
  await render(
    <AccountSuspension
      user={user}
      session={session}
      onUpdated={(value) => updated.push(value)}
      onAccessFailure={() => {}}
    />,
  );
  await click(button(document.body, "停用账户"));
  await click(button(alertDialog(), "停用账户"));
  expect(button(alertDialog(), "正在处理").disabled).toBe(true);
  expect(calls.filter((call) => call.init?.method === "POST")).toHaveLength(1);
  const signal = calls.find((call) => call.init?.method === "POST")?.init
    ?.signal;
  await cleanup();
  expect(signal?.aborted).toBe(true);
  await act(async () => finish?.(json({ ...user, suspended: true })));
  await settle();
  expect(updated).toHaveLength(0);
});

test("概览展示实际存储、两类预警及账户用量，切换英文保留用户内容", async () => {
  const host = await render(<App />);
  await waitFor(
    () => host.textContent?.includes("数据库（含日志）") === true,
    "storage overview",
  );
  expect(host.textContent).toContain("800 B / 1,000 B");
  expect(host.textContent).toContain("4 KiB");
  expect(host.textContent).toContain("磁盘可用");
  expect(host.textContent).toContain("预警线（80%）");
  expect(host.textContent).toContain("磁盘可用空间低于 2 GiB");
  expect(
    host.querySelector('[role="progressbar"]')?.getAttribute("aria-valuenow"),
  ).toBe("80");
  expect(host.querySelector("tbody")?.textContent).toContain("700 B");
  const count = calls.length;
  await act(async () => setLanguage("en"));
  expect(calls).toHaveLength(count);
  expect(host.textContent).toContain("Database and journal");
  expect(host.textContent).toContain("成员<测试>");
});

test("无法测量的存储值保持未知，不伪装为零字节", async () => {
  await act(async () => setLanguage("en"));
  const host = await render(
    <StorageOverview
      storage={{ ...storage, databaseBytes: null, diskAvailableBytes: null }}
    />,
  );
  expect(host.textContent?.match(/Unavailable/g)).toHaveLength(4);
  expect(host.textContent).not.toContain("NaN");
  expect(
    [...host.querySelectorAll("dd")].map((value) => value.textContent),
  ).not.toContain("0 B");
});
