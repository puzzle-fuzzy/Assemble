import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import { App } from "./App";
import type { AdminDriveFile, AdminStorage } from "./lib/api";
import { adminDriveDownloadPath } from "./lib/api";
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

const GiB = 1024 ** 3;
const storage: AdminStorage = {
  contentBytes: 256 * GiB,
  budgetBytes: 1024 * GiB,
  diskAvailableBytes: 64 * GiB,
  databaseBytes: 2 * GiB,
  warningPercent: 80,
  budgetWarning: false,
  diskWarning: false,
  topAccounts: [],
};
const files: AdminDriveFile[] = [
  {
    id: "file-1",
    accountId: "owner-1",
    accountDisplayName: "文件所有者",
    accountEmail: "owner@example.test",
    name: "季度报表.pdf",
    size: 1536,
    mimeType: "application/pdf",
    createdAt: "2026-11-26T01:02:03Z",
  },
  {
    id: "file-2",
    accountId: "owner-2",
    accountDisplayName: "另一位用户",
    accountEmail: null,
    name: "很长的文件名称".repeat(20),
    size: 0,
    mimeType: null,
    createdAt: "2026-11-25T01:02:03Z",
  },
];
const originalFetch = globalThis.fetch;
const requests: { path: string; init?: RequestInit }[] = [];
let handle: (path: string, init?: RequestInit) => Response | Promise<Response>;
const json = (value: unknown, status = 200) => Response.json(value, { status });
function listing(path: string, items = files) {
  const parameters = new URL(path, "http://localhost").searchParams;
  return {
    items,
    total: items.length ? 21 : 0,
    page: Number(parameters.get("page") || 1),
    pageSize: Number(parameters.get("pageSize") || 20),
  };
}
function response(path: string): Response {
  if (path === "/v1/admin/overview")
    return json({
      users: 2,
      devices: 0,
      onlineDevices: 0,
      registrationsToday: 0,
      storage,
    });
  if (path === "/v1/admin/session")
    return json({
      account: {
        id: "admin",
        displayName: "管理员",
        email: "admin@example.test",
        avatarUrl: null,
      },
      csrfToken: "test-only",
    });
  if (path === "/v1/admin/storage") return json(storage);
  if (path.startsWith("/v1/admin/drive/files?")) return json(listing(path));
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
  setLanguage("zh-CN");
});

async function renderDrive() {
  const host = await render(<App />);
  await navigate(host, "drive");
  return host;
}

test("云盘显示全站文件与真实容量，搜索分页、详情弹窗和双语切换保持列表", async () => {
  const host = await renderDrive();
  const initialRequest = requests.find((request) =>
    request.path.startsWith("/v1/admin/drive/files?"),
  );
  expect(
    new URL(initialRequest?.path ?? "", "http://localhost").searchParams.get(
      "pageSize",
    ),
  ).toBe("20");
  const downloads = [
    ...host.querySelectorAll<HTMLAnchorElement>("a[download]"),
  ];
  expect(downloads).toHaveLength(files.length);
  expect(downloads[0]?.getAttribute("href")).toBe(
    "/v1/admin/drive/files/file-1/content",
  );
  expect(downloads[0]?.textContent).toBe("下载");
  expect(downloads[0]?.getAttribute("aria-label")).toContain(files[0].name);
  expect(downloads[0]?.closest("tr")?.textContent).toContain("详情");
  expect(
    host.querySelector('[aria-current="page"][href="#drive"]'),
  ).not.toBeNull();
  expect(host.querySelector('[data-slot="breadcrumb-page"]')?.textContent).toBe(
    "云盘",
  );
  expect(host.textContent).toContain("文件所有者");
  expect(host.textContent).toContain("另一位用户");
  expect(host.textContent).toContain("未绑定邮箱");
  expect(host.textContent).toContain("未记录");
  expect(
    [...host.querySelectorAll('section[aria-label="服务器存储空间"] dd')].map(
      (value) => value.textContent,
    ),
  ).toEqual(["64 GiB", "64 GiB", "256 GiB / 1 TiB", "2 GiB"]);
  expect(host.querySelector("time")?.getAttribute("datetime")).toBe(
    files[0].createdAt,
  );
  await click(button(host, "下一页"));
  expect(requests.at(-1)?.path).toContain("page=2");
  await search(host, " 报表+第一版@example.test ");
  const query = new URL(requests.at(-1)?.path ?? "", "http://localhost")
    .searchParams;
  expect(query.get("q")).toBe("报表+第一版@example.test");
  expect(query.get("page")).toBe("1");
  const trigger = button(host, "详情：季度报表.pdf");
  await act(async () => trigger.focus());
  await click(trigger);
  const dialog = document.body.querySelector<HTMLElement>('[role="dialog"]');
  expect(dialog?.textContent).toContain("文件详情");
  expect(dialog?.textContent).toContain("application/pdf");
  expect(host.querySelector("tbody")?.textContent).toContain(files[0].name);
  expect(window.location.hash).toBe("#drive");
  const before = requests.length;
  await act(async () => setLanguage("en"));
  expect(dialog?.textContent).toContain("File details");
  expect(host.querySelector("a[download]")?.textContent).toBe("Download");
  expect(host.textContent).toContain("Available file storage");
  expect(requests).toHaveLength(before);
  await click(button(document.body, "Close"));
  expect(document.activeElement === trigger).toBe(true);
  expect(
    host.querySelector<HTMLInputElement>('input[type="search"]')?.value,
  ).toBe(" 报表+第一版@example.test ");
  await click(button(host, "Refresh"));
  expect(
    requests.filter(({ path }) => path === "/v1/admin/storage"),
  ).toHaveLength(2);
  for (const request of requests) {
    expect(request.init?.credentials).toBe("same-origin");
    expect(request.init?.cache).toBe("no-store");
    expect(request.init?.signal).toBeInstanceOf(AbortSignal);
  }
});

test("管理员下载只生成同源文件标识地址，不把特殊字符变成路径或查询参数", () => {
  expect(adminDriveDownloadPath("file/中文?#")).toBe(
    "/v1/admin/drive/files/file%2F%E4%B8%AD%E6%96%87%3F%23/content",
  );
});

test.each([
  { disk: 900 * GiB, content: 256 * GiB, expected: "768 GiB" },
  { disk: 0, content: 256 * GiB, expected: "0 B" },
  { disk: 900 * GiB, content: 2048 * GiB, expected: "0 B" },
  { disk: null, content: 256 * GiB, expected: "暂不可用" },
])(
  "空间取预算和磁盘的较小值，不把未知容量伪装为零：%j",
  async ({ disk, content, expected }) => {
    handle = (path) =>
      path === "/v1/admin/storage"
        ? json({
            ...storage,
            diskAvailableBytes: disk,
            contentBytes: content,
            databaseBytes: null,
          })
        : response(path);
    const host = await renderDrive();
    expect(
      host.querySelector('section[aria-label="服务器存储空间"] dd')
        ?.textContent,
    ).toBe(expected);
    expect(
      [...host.querySelectorAll('section[aria-label="服务器存储空间"] dd')].at(
        -1,
      )?.textContent,
    ).toBe("暂不可用");
  },
);

test.each([404, 503])(
  "目录读取错误 %s 可重试，搜索空结果使用真实空态",
  async (status) => {
    let failing = true;
    handle = (path) =>
      path.startsWith("/v1/admin/drive/files?")
        ? failing
          ? json({ diagnostic: "private response body" }, status)
          : json(listing(path, []))
        : response(path);
    const host = await renderDrive();
    expect(host.textContent).toContain("暂时无法读取数据");
    expect(host.textContent).not.toContain("private response body");
    failing = false;
    await click(button(host, "重试"));
    expect(host.textContent).toContain("还没有云盘文件");
    await search(host, "不存在的文件");
    expect(host.textContent).toContain("没有匹配的结果");
    expect(button(host, "下一页").disabled).toBe(true);
  },
);

test.each([401, 403])(
  "云盘读取失去权限 %s 时立即移除文件和容量",
  async (status) => {
    handle = (path) =>
      path.startsWith("/v1/admin/drive/files?")
        ? json({ diagnostic: "private body" }, status)
        : response(path);
    const host = await renderDrive();
    expect(host.textContent).toContain(
      status === 401 ? "登录管理控制台" : "没有管理权限",
    );
    expect(host.textContent).not.toContain(files[0].name);
    expect(host.textContent).not.toContain("64 GiB");
    expect(host.textContent).not.toContain("private body");
  },
);

test("迟到的旧搜索不能覆盖新目录", async () => {
  let resolveOld: ((value: Response) => void) | undefined;
  let oldSignal: AbortSignal | null | undefined;
  handle = (path, init) => {
    if (
      path.startsWith("/v1/admin/drive/files?") &&
      new URL(path, "http://localhost").searchParams.get("q") === "旧查询"
    ) {
      oldSignal = init?.signal;
      return new Promise((resolve) => {
        resolveOld = resolve;
      });
    }
    return response(path);
  };
  const host = await renderDrive();
  await search(host, "旧查询");
  await search(host, "新查询");
  expect(oldSignal?.aborted).toBe(true);
  await act(async () =>
    resolveOld?.(
      json(listing("http://localhost", [{ ...files[0], name: "旧结果文件" }])),
    ),
  );
  await settle();
  await waitForText(host, files[0].name);
  expect(host.textContent).not.toContain("旧结果文件");
});
