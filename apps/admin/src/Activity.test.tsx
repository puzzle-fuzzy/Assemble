import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import mediaContract from "../../../packages/media/contract/snapshots.json";
import { chooseSelect } from "../../../packages/ui/src/test-support/select";
import contract from "../contract/activity.json";
import { App } from "./App";
import { ActivityFileFocus } from "./components/activity-file-focus";
import { ActivityPage } from "./components/activity-page";
import { MediaPage } from "./components/media-page";
import type { AdminActivity, AdminActivityTarget } from "./lib/activity-api";
import { readActivityTarget, subjectHref } from "./lib/activity-api";
import {
  button,
  cleanup,
  click,
  navigate,
  render,
  waitFor,
  waitForText,
} from "./test-support/dom";

const events: AdminActivity[] = contract.events as AdminActivity[];
const targets: AdminActivityTarget[] =
  contract.targets as AdminActivityTarget[];
function at<T>(items: T[], index: number): T {
  const item = items[index];
  if (item === undefined) throw new Error("FIXTURE_MISSING");
  return item;
}
const session = {
  account: {
    id: "admin-1",
    displayName: "管理员",
    email: "admin@example.test",
    avatarUrl: null,
  },
  csrfToken: "test-only",
};
const originalFetch = globalThis.fetch;
const requests: { path: string; init?: RequestInit }[] = [];
let handle: (path: string, init?: RequestInit) => Response | Promise<Response>;
const json = (data: unknown, status = 200) => Response.json(data, { status });
const noFailure = () => {};
function response(path: string): Response {
  if (path === "/v1/admin/session") return json(session);
  if (path === "/v1/admin/overview")
    return json({
      users: 1,
      devices: 2,
      onlineDevices: 0,
      registrationsToday: 0,
    });
  if (path.startsWith("/v1/admin/activity?")) {
    const query = new URL(path, "http://localhost").searchParams;
    return json({
      items: events,
      total: 41,
      page: Number(query.get("page") ?? 1),
      pageSize: Number(query.get("pageSize") ?? 20),
    });
  }
  const target = targets.find(
    (item) =>
      path ===
      `/v1/admin/activity/targets/${item.subject.kind}/${item.subject.id}`,
  );
  if (target) return json(target);
  if (path === "/v1/admin/drive/files/object-1")
    return json({
      id: "object-1",
      accountId: "account-1",
      accountDisplayName: "所有者",
      accountEmail: null,
      name: "合同.txt",
      size: 20,
      mimeType: "text/plain",
      createdAt: events[0]?.occurredAt,
    });
  if (path === "/v1/media/capabilities")
    return json({
      providers: { tmdb: false, douban: false },
      canMaintainShared: true,
      canMaintainPrivate: true,
    });
  if (path.startsWith("/v1/admin/media/entries?"))
    return json({ ...mediaContract.listing, items: [], total: 0 });
  if (path === "/v1/admin/media/entries/object-3")
    return json({
      item: {
        ...mediaContract.entries[0],
        id: "object-3",
        title: "日志中的准确影片",
      },
    });
  if (path === "/v1/admin/media/entries/object-3/gallery")
    return json({ entryId: "object-3", version: 1, items: [] });
  if (path.startsWith("/v1/media/collections?"))
    return json({ items: [], total: 0, page: 1, pageSize: 12 });
  if (path === "/v1/media/collections/object-4")
    return json({
      item: {
        id: "object-4",
        version: 1,
        title: "日志中的准确合集",
        description: "",
        poster: null,
        items: [],
        createdAt: events[0]?.occurredAt,
        updatedAt: events[0]?.occurredAt,
      },
    });
  return json({}, 404);
}
beforeEach(() => {
  setLanguage("zh-CN");
  document.body.replaceChildren();
  window.history.replaceState(null, "", "/admin/");
  requests.length = 0;
  handle = response;
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
async function hash(value: string) {
  await act(async () => {
    window.history.pushState(null, "", value);
    window.dispatchEvent(new Event("hashchange"));
  });
}

test("契约覆盖四类业务、五类对象和缺少历史字段的旧记录", () => {
  expect(events).toHaveLength(6);
  expect(targets).toHaveLength(5);
  expect(new Set(events.slice(0, 5).map((item) => item.category)).size).toBe(4);
  expect(events[5]?.snapshot).toBeNull();
  for (const target of targets)
    expect(
      readActivityTarget(
        `activity/${target.subject.kind}/${target.subject.id}`,
      ),
    ).toEqual({ kind: target.subject.kind, id: target.subject.id });
  expect(readActivityTarget("activity/remote/%")).toBeNull();
  expect(readActivityTarget("activity/remote/id/extra")).toBeNull();
  expect(subjectHref({ ...at(targets, 0).subject, available: false })).toBe(
    "#activity/file/object-1",
  );
});

test("管理菜单显示操作人、设备、北京时间和安全的对象链接，支持中英切换", async () => {
  const host = await render(<App />);
  await navigate(host, "activity");
  await waitForText(host, "合同.txt");
  expect(host.textContent).toContain("测试账户");
  expect(host.textContent).toContain("Windows");
  expect(host.textContent).toContain("16:00");
  for (const href of [
    "#drive?file=object-1",
    "#media?entry=object-3",
    "#media?collection=object-4",
    "#activity/remote/object-5",
    "#activity/transfer/object-2",
  ])
    expect(host.querySelector(`a[href="${href}"]`)).not.toBeNull();
  expect(
    host.querySelector(
      'select:not([aria-hidden="true"]),input[type="date"],input[type="datetime-local"]',
    ),
  ).toBeNull();
  expect(
    host.querySelector('nav a[href="#activity"]')?.getAttribute("aria-current"),
  ).toBe("page");
  await act(async () => setLanguage("en"));
  expect(host.textContent).toContain("Activity log");
  expect(host.textContent).toContain("Uploaded file");
});

test("筛选和分页明确提交，刷新保持筛选，返回日志保留原条件", async () => {
  const host = await render(<App />);
  await navigate(host, "activity");
  await chooseSelect(host, "操作类型", "远程连接");
  const input = host.querySelector<HTMLInputElement>('input[type="search"]');
  if (!input) throw new Error("SEARCH_MISSING");
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set?.call(input, "Mac");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await click(button(host, "筛选"));
  await waitFor(
    () =>
      requests.some(
        ({ path }) =>
          path.includes("q=Mac") && path.includes("category=remote"),
      ),
    "筛选请求",
  );
  const before = requests.filter(({ path }) =>
    path.startsWith("/v1/admin/activity?"),
  ).length;
  await click(button(host, "刷新"));
  await waitFor(
    () =>
      requests.filter(({ path }) => path.startsWith("/v1/admin/activity?"))
        .length ===
      before + 1,
    "刷新请求",
  );
  await hash("#activity/remote/object-5");
  await waitForText(host, "近期记录");
  await hash("#activity");
  expect(
    host.querySelector<HTMLInputElement>('input[type="search"]')?.value,
  ).toBe("Mac");
  const next = [...host.querySelectorAll<HTMLButtonElement>("button")].find(
    (item) =>
      item.textContent?.includes("下一页") ||
      item.getAttribute("aria-label") === "下一页",
  );
  if (!next) throw new Error("缺少分页按钮");
  await click(next);
  await waitFor(
    () =>
      requests.some(
        ({ path }) => path.includes("page=2") && path.includes("q=Mac"),
      ),
    "第二页",
  );
  await click(button(host, "重置"));
  await waitFor(
    () =>
      new URL(
        requests.at(-1)?.path ?? "/",
        "http://localhost",
      ).searchParams.get("q") === "",
    "重置请求",
  );
});

test("历史对象详情支持已删除和旧快照缺失，不显示错误对象或迟到结果", async () => {
  let finish: ((response: Response) => void) | undefined;
  handle = (path) =>
    path.endsWith("/remote/slow")
      ? new Promise((resolve) => {
          finish = resolve;
        })
      : response(path);
  const host = await render(<App />);
  await hash("#activity/remote/slow");
  await waitFor(() => finish !== undefined, "慢详情请求已发出");
  await hash("#activity/file/object-1");
  await waitForText(host, "文件类型");
  await act(async () =>
    finish?.(
      json({
        ...targets[4],
        subject: {
          ...at(targets, 4).subject,
          id: "slow",
          name: "过时远程对象",
        },
      }),
    ),
  );
  expect(host.textContent).not.toContain("过时远程对象");
  await cleanup();
  const missing = {
    ...at(targets, 0),
    subject: { ...at(targets, 0).subject, available: false },
    snapshot: null,
  };
  handle = (path) =>
    path.endsWith("/file/object-1") ? json(missing) : response(path);
  const second = await render(
    <ActivityPage
      target={{ kind: "file", id: "object-1" }}
      onAccessFailure={noFailure}
    />,
  );
  await waitForText(second, "原日志未保存对象快照");
  expect(second.textContent).toContain("原数据已删除");
});

test("文件深链接按唯一标识读取，读取失败不会改打开列表里的其他文件", async () => {
  const host = await render(
    <ActivityFileFocus
      id="object-1"
      onClose={noFailure}
      onAccessFailure={noFailure}
    />,
  );
  await waitForText(document.body, "合同.txt");
  expect(
    requests.some(({ path }) => path === "/v1/admin/drive/files/object-1"),
  ).toBe(true);
  expect(document.body.querySelector('[role="dialog"]')?.textContent).toContain(
    "所有者",
  );
  await cleanup();
  handle = () => json({}, 404);
  await render(
    <ActivityFileFocus
      id="deleted-file"
      onClose={noFailure}
      onAccessFailure={noFailure}
    />,
  );
  await waitFor(
    () => document.body.textContent?.includes("重试") === true,
    "缺失对象错误",
  );
  expect(document.body.textContent).not.toContain("合同.txt");
  expect(host.isConnected).toBe(false);
});

test.each(["entry", "collection"] as const)(
  "影视 %s 深链接不依赖当前分页，打开准确详情",
  async (kind) => {
    const host = await render(
      <MediaPage
        session={session}
        onAccessFailure={noFailure}
        focusEntryId={kind === "entry" ? "object-3" : null}
        focusCollectionId={kind === "collection" ? "object-4" : null}
      />,
    );
    await waitForText(
      document.body,
      kind === "entry" ? "日志中的准确影片" : "日志中的准确合集",
    );
    expect(
      document.body.querySelector('[role="dialog"],[role="complementary"]'),
    ).not.toBeNull();
    expect(
      requests.some(
        ({ path }) =>
          path ===
          (kind === "entry"
            ? "/v1/admin/media/entries/object-3"
            : "/v1/media/collections/object-4"),
      ),
    ).toBe(true);
    expect(host.isConnected).toBe(true);
  },
);
