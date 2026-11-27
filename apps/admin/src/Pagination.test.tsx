import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import dht from "../contract/dht.json";
import { App } from "./App";
import {
  adminLocation,
  button,
  cleanup,
  click,
  render,
  search,
  waitFor,
} from "./test-support/dom";

const originalFetch = globalThis.fetch;
let restoreLocation: () => void;
const requests: string[] = [];
let total = 300;
const endpoints = {
  users: "/v1/admin/users",
  devices: "/v1/admin/devices",
  logins: "/v1/admin/logins",
  drive: "/v1/admin/drive/files",
  dht: "/v1/admin/dht/observations",
  media: "/v1/admin/media/entries",
  collections: "/v1/media/collections",
};
type List = keyof typeof endpoints;

function latest(list: List) {
  const path = [...requests]
    .reverse()
    .find((path) => path.startsWith(`${endpoints[list]}?`));
  return path ? new URL(path, "http://localhost").searchParams : null;
}
async function loaded(list: List, page: number) {
  await waitFor(
    () => latest(list)?.get("page") === String(page),
    `${list} 第 ${page} 页`,
  );
}
function locationQuery() {
  return new URLSearchParams(window.location.hash.split("?")[1]);
}

beforeEach(async () => {
  await setLanguage("zh-CN");
  restoreLocation = adminLocation();
  requests.length = 0;
  total = 300;
  globalThis.fetch = mock(async (input: RequestInfo | URL) => {
    const path = String(input);
    requests.push(path);
    if (path === "/v1/admin/session")
      return Response.json({
        account: {
          id: "admin",
          displayName: "测试",
          email: "admin@example.test",
          avatarUrl: null,
        },
        csrfToken: "synthetic-only",
      });
    if (path === "/v1/admin/dht/status") return Response.json(dht.status);
    if (path === "/v1/media/capabilities")
      return Response.json({
        providers: { tmdb: false, douban: false },
        canMaintainShared: true,
        canMaintainPrivate: true,
      });
    if (
      Object.values(endpoints).some((endpoint) =>
        path.startsWith(`${endpoint}?`),
      )
    ) {
      const params = new URL(path, "http://localhost").searchParams;
      return Response.json({
        items: [],
        total,
        page: Number(params.get("page")),
        pageSize: Number(params.get("pageSize")),
        counts: { all: total, planned: total, watching: 0, completed: 0 },
      });
    }
    return Response.json({}, { status: 503 });
  }) as unknown as typeof fetch;
});
afterEach(async () => {
  await cleanup();
  restoreLocation();
  globalThis.fetch = originalFetch;
});

test.each(["users", "devices", "logins", "drive", "dht", "media"] as const)(
  "%s 从地址首次请求指定页，翻页后重新挂载仍请求同一页",
  async (list) => {
    window.history.replaceState(
      null,
      "",
      `/admin/#${list}?page=3&pageSize=10&q=中文%2B%26%3D`,
    );
    const host = await render(<App />);
    await loaded(list, 3);
    expect(latest(list)?.get("pageSize")).toBe("10");
    expect(latest(list)?.get("q")).toBe("中文+&=");
    await waitFor(() => !button(host, "下一页").disabled, "分页可用");
    await click(button(host, "下一页"));
    await loaded(list, 4);
    expect(locationQuery().get("page")).toBe("4");
    expect(locationQuery().get("q")).toBe("中文+&=");
    await cleanup();
    requests.length = 0;
    const restored = await render(<App />);
    await loaded(list, 4);
    expect(
      requests.filter((path) => path.startsWith(`${endpoints[list]}?`)),
    ).toHaveLength(1);
    expect(
      restored.querySelector<HTMLInputElement>('input[type="search"]')?.value,
    ).toBe("中文+&=");
    expect(locationQuery().has("csrf")).toBe(false);
  },
);

test("DHT 筛选与浏览器后退前进同步，搜索重置页码，刷新保留新查询", async () => {
  window.history.replaceState(
    null,
    "",
    "/admin/#dht?page=3&pageSize=10&status=all&source=sample&q=原查询",
  );
  const host = await render(<App />);
  await loaded("dht", 3);
  expect(latest("dht")?.has("status")).toBe(false);
  expect(latest("dht")?.get("source")).toBe("sample");
  await waitFor(() => !button(host, "下一页").disabled, "分页可用");
  await click(button(host, "下一页"));
  await loaded("dht", 4);
  await act(async () => window.history.back());
  await loaded("dht", 3);
  expect(locationQuery().get("page")).toBe("3");
  await act(async () => window.history.forward());
  await loaded("dht", 4);
  await search(host, "新查询+&=");
  await loaded("dht", 1);
  expect(locationQuery().get("q")).toBe("新查询+&=");
  expect(locationQuery().get("status")).toBe("all");
  expect(locationQuery().get("source")).toBe("sample");
  await act(async () => window.history.back());
  await loaded("dht", 4);
  expect(
    host.querySelector<HTMLInputElement>('input[type="search"]')?.value,
  ).toBe("原查询");
});

test("影视合集恢复页签与独立分页，不误读片库页码", async () => {
  window.history.replaceState(
    null,
    "",
    "/admin/#media?tab=collections&page=7&collectionPage=3&collectionPageSize=30",
  );
  const host = await render(<App />);
  await loaded("collections", 3);
  expect(latest("collections")?.get("pageSize")).toBe("30");
  expect(latest("media")).toBeNull();
  await waitFor(() => !button(host, "下一页").disabled, "合集分页可用");
  await click(button(host, "下一页"));
  await loaded("collections", 4);
  expect(locationQuery().get("page")).toBe("7");
  await cleanup();
  requests.length = 0;
  await render(<App />);
  await loaded("collections", 4);
  expect(latest("media")).toBeNull();
});

test("DHT 更改列筛选一次性回第一页，并把筛选写入地址", async () => {
  window.history.replaceState(
    null,
    "",
    "/admin/#dht?page=3&pageSize=10&status=all&source=sample&q=筛选",
  );
  const host = await render(<App />);
  await loaded("dht", 3);
  const before = requests.length;
  await act(async () => {
    button(host, "解析状态: 全部").dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
    );
  });
  await waitFor(
    () => document.querySelector('[role="menuitemradio"]') !== null,
    "状态筛选菜单",
  );
  const verified = [
    ...document.querySelectorAll<HTMLElement>('[role="menuitemradio"]'),
  ].find((item) => item.textContent === "已验证");
  if (!verified) throw new Error("找不到已验证筛选");
  await click(verified);
  await loaded("dht", 1);
  const reads = requests
    .slice(before)
    .filter((path) => path.startsWith(`${endpoints.dht}?`));
  expect(reads).toHaveLength(1);
  expect(latest("dht")?.get("status")).toBe("verified");
  expect(latest("dht")?.get("source")).toBe("sample");
  expect(locationQuery().has("page")).toBe(false);
  await cleanup();
  requests.length = 0;
  await render(<App />);
  await loaded("dht", 1);
  expect(latest("dht")?.get("status")).toBe("verified");
  expect(latest("dht")?.get("q")).toBe("筛选");
});

test("影视网址恢复片库的类型、观看状态与所属筛选", async () => {
  window.history.replaceState(
    null,
    "",
    "/admin/#media?page=3&mediaType=tv&status=watching&scope=private&sort=title&q=星球",
  );
  const host = await render(<App />);
  await loaded("media", 3);
  for (const [key, value] of Object.entries({
    mediaType: "tv",
    status: "watching",
    scope: "private",
    sort: "title",
    q: "星球",
  }))
    expect(latest("media")?.get(key)).toBe(value);
  expect(button(host, "作品类型: 剧集")).toBeDefined();
  await search(host, "星际");
  await loaded("media", 1);
  expect(locationQuery().get("scope")).toBe("private");
  expect(locationQuery().get("q")).toBe("星际");
});

test("失效页码回退到末页并替换地址，不制造后退循环", async () => {
  total = 45;
  window.history.replaceState(null, "", "/admin/#dht?page=99&pageSize=10");
  const length = window.history.length;
  await render(<App />);
  await loaded("dht", 5);
  expect(locationQuery().get("page")).toBe("5");
  expect(window.history.length).toBe(length);
});

test("非法地址参数回到有效默认值，保持 DHT 默认已验证筛选", async () => {
  window.history.replaceState(
    null,
    "",
    "/admin/#dht?page=-3&pageSize=100000&status=unknown&source=unknown",
  );
  await render(<App />);
  await loaded("dht", 1);
  expect(latest("dht")?.get("pageSize")).toBe("20");
  expect(latest("dht")?.get("status")).toBe("verified");
  expect(latest("dht")?.has("source")).toBe(false);
});
