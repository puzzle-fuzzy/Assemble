import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import type { MediaEntry, MediaSession } from "@assemble/media";
import { act } from "react";
import { App } from "./App";
import { discardHomePreload, preloadAnonymousHome } from "./lib/home-preload";
import { mediaSiteResources } from "./lib/i18n";
import { navigate, readRoute, wallPath } from "./lib/navigation";
import { button, cleanup, click, render, settle } from "./test-support/dom";

const entry: MediaEntry = {
  id: "film-1",
  version: 1,
  scope: "shared",
  mediaType: "movie",
  title: "真实片库测试作品",
  originalTitle: "Test title",
  year: 2026,
  releaseDate: "2026-11-25",
  overview: "测试简介",
  genres: ["剧情"],
  episodeCount: null,
  status: "watching",
  watchedAt: null,
  review: null,
  notes: null,
  magnetUri: null,
  cast: [
    {
      name: "演员甲",
      character: "角色甲",
      identity: { provider: "tmdb", externalId: "42" },
      photo: null,
    },
  ],
  identities: [{ provider: "tmdb", mediaType: "movie", externalId: "99" }],
  poster: null,
  magnetAvailable: true,
  createdAt: "2026-11-25T00:00:00Z",
  updatedAt: "2026-11-25T00:00:00Z",
};
const account = { id: "account-a", displayName: "测试账户", avatarUrl: null };
const originalFetch = globalThis.fetch;
const requests: { path: string; init?: RequestInit }[] = [];
let session: MediaSession;
let previousUrl = "about:blank";
let handle: (path: string, init?: RequestInit) => Response | Promise<Response>;

function json(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}
function list(items = [entry], page = 1, total = items.length) {
  return {
    items,
    total,
    page,
    pageSize: 12,
    counts: { all: total, planned: 0, watching: total, completed: 0 },
  };
}
function response(path: string) {
  const url = new URL(path, "http://localhost");
  if (url.pathname === "/v1/media/session") return json(session);
  if (url.pathname === "/v1/media/shared/entries")
    return json(list([entry], Number(url.searchParams.get("page") ?? "1")));
  if (url.pathname === "/v1/media/private/entries")
    return json(
      list([{ ...entry, scope: "private", title: "账户甲的私人作品" }]),
    );
  if (url.pathname === "/v1/media/shared/entries/film-1")
    return json({ item: entry });
  if (url.pathname === "/v1/media/shared/entries/film-1/gallery")
    return json({ entryId: entry.id, version: 1, items: [] });
  if (url.pathname === "/v1/media/catalog/actors/tmdb/42")
    return json({
      actor: {
        identity: { provider: "tmdb", externalId: "42" },
        name: "演员甲",
        photo: null,
      },
      items: [
        {
          identity: entry.identities[0],
          title: "来源中的作品",
          originalTitle: "Source title",
          year: 2025,
          overview: "来源简介",
          poster: null,
        },
      ],
    });
  if (url.pathname === "/v1/media/catalog/details")
    return json({
      item: {
        identity: entry.identities[0],
        title: "来源中的作品",
        originalTitle: "Source title",
        year: 2025,
        overview: "来源简介",
        poster: null,
      },
    });
  if (url.pathname === "/v1/media/collections")
    return json({
      items: [
        {
          id: "collection-1",
          version: 1,
          title: "周末片单",
          description: "共享收藏",
          poster: null,
          items: [entry],
          createdAt: entry.createdAt,
          updatedAt: entry.updatedAt,
        },
      ],
      total: 1,
      page: 1,
      pageSize: 12,
    });
  if (url.pathname === "/v1/media/collections/collection-1")
    return json({
      item: {
        id: "collection-1",
        version: 1,
        title: "周末片单",
        description: "共享收藏",
        poster: null,
        items: [entry],
        createdAt: entry.createdAt,
        updatedAt: entry.updatedAt,
      },
    });
  return json({ code: "NOT_FOUND", detail: "untrusted-sensitive-detail" }, 404);
}

beforeEach(() => {
  setLanguage("zh-CN");
  document.body.replaceChildren();
  previousUrl = window.location.href;
  (
    window as unknown as { happyDOM: { setURL: (url: string) => void } }
  ).happyDOM.setURL("http://localhost/movies/");
  session = { account: null };
  requests.length = 0;
  handle = response;
  globalThis.fetch = mock((input: RequestInfo | URL, init?: RequestInit) => {
    const path = String(input);
    requests.push({ path, init });
    return Promise.resolve(handle(path, init));
  }) as unknown as typeof fetch;
});

afterEach(async () => {
  discardHomePreload();
  await cleanup();
  globalThis.fetch = originalFetch;
  setLanguage("zh-CN");
  (
    window as unknown as { happyDOM: { setURL: (url: string) => void } }
  ).happyDOM.setURL(previousUrl);
});

describe("影视展示端", () => {
  test.each([false, true])(
    "首屏只向确认匿名的会话提供公开预取，登录状态 %s",
    async (signedIn) => {
      session = { account: signedIn ? account : null };
      const previousFetch = window.fetch;
      window.fetch = globalThis.fetch;
      try {
        preloadAnonymousHome();
        const host = await render(<App />);
        expect(host.textContent).toContain(entry.title);
        const lists = requests.filter(
          ({ path }) =>
            new URL(path, "http://localhost").pathname ===
            "/v1/media/shared/entries",
        );
        expect(lists).toHaveLength(signedIn ? 6 : 3);
        expect(
          lists.filter(({ init }) => init?.credentials === "omit"),
        ).toHaveLength(3);
        if (signedIn) expect(window.assembleMediaHomePreload).toBeUndefined();
      } finally {
        window.fetch = previousFetch;
      }
    },
  );

  test("首屏预取中重新核验会话会取消旧请求，并恢复当前会话的加载", async () => {
    handle = (path, init) =>
      new URL(path, "http://localhost").pathname ===
        "/v1/media/shared/entries" && init?.credentials === "omit"
        ? new Promise<Response>(() => {})
        : response(path);
    const previousFetch = window.fetch;
    window.fetch = globalThis.fetch;
    try {
      preloadAnonymousHome();
      const host = await render(<App />);
      expect(host.textContent).not.toContain(entry.title);
      await act(async () => window.dispatchEvent(new Event("focus")));
      await settle();
      expect(host.textContent).toContain(entry.title);
      expect(host.textContent).not.toContain("暂时无法载入");
      const preloadRequests = requests.filter(
        ({ init }) => init?.credentials === "omit",
      );
      expect(preloadRequests).toHaveLength(3);
      expect(preloadRequests.every(({ init }) => init?.signal?.aborted)).toBe(
        true,
      );
    } finally {
      window.fetch = previousFetch;
    }
  });

  test("匿名首页只有类型切换和三种状态海报分区，没有header和管理筛选", async () => {
    const host = await render(<App />);
    expect(host.textContent).toContain(entry.title);
    expect(host.querySelector("header")).toBeNull();
    expect(host.querySelector("form, input, select")).toBeNull();
    expect(host.querySelectorAll(".home-shelf")).toHaveLength(3);
    expect(
      host.querySelectorAll(".poster-card [data-slot=badge]"),
    ).toHaveLength(0);
    expect(host.textContent).not.toContain("在这里浏览");
    const statuses = requests
      .filter(({ path }) => path.startsWith("/v1/media/shared/entries?"))
      .map(({ path }) =>
        new URL(path, "http://localhost").searchParams.get("status"),
      );
    expect(statuses).toEqual(["planned", "watching", "completed"]);
    expect(
      host.querySelector('a[href="/web/login?next=/movies/"]'),
    ).not.toBeNull();
    expect(host.querySelector('input[type="password"]')).toBeNull();
    expect(
      requests.every(
        ({ init }) =>
          init?.method === "GET" && init.credentials === "same-origin",
      ),
    ).toBe(true);
    expect(requests.some(({ path }) => path.includes("private"))).toBe(false);
  });

  test("类型一次切换三个分区，各自服务端分页且详情返回保留位置", async () => {
    window.history.replaceState(
      null,
      "",
      "/movies/?plannedPage=2&watchingPage=3&completedPage=2",
    );
    handle = (path) =>
      path.startsWith("/v1/media/shared/entries?")
        ? json(
            list(
              [entry],
              Number(
                new URL(path, "http://localhost").searchParams.get("page") ?? 1,
              ),
              48,
            ),
          )
        : response(path);
    const host = await render(<App />);
    await click(button(host, "剧集"));
    expect(window.location.search).toBe("?mediaType=tv");
    for (const { path } of requests.slice(-3)) {
      const params = new URL(path, "http://localhost").searchParams;
      expect(params.get("mediaType")).toBe("tv");
      expect(params.get("page")).toBe("1");
      expect(params.get("pageSize")).toBe("12");
      expect(params.get("sort")).toBe(
        params.get("status") === "completed" ? "watched" : "updated",
      );
      expect(params.has("q") || params.has("genre") || params.has("year")).toBe(
        false,
      );
    }
    const planned = host.querySelector<HTMLElement>(
      '[aria-labelledby="shelf-planned"]',
    );
    if (!planned) throw new Error("SHELF_MISSING");
    await click(button(planned, "下一页"));
    expect(window.location.search).toContain("plannedPage=2");
    expect(window.location.search).not.toContain("watchingPage");
    const link = planned.querySelector<HTMLAnchorElement>("h3 a");
    if (!link) throw new Error("FILM_LINK_MISSING");
    await click(link);
    const back = host.querySelector<HTMLAnchorElement>(
      'a[href="/movies/?mediaType=tv&plannedPage=2"]',
    );
    if (!back) throw new Error("BACK_LINK_MISSING");
    await click(back);
    expect(window.location.search).toBe("?mediaType=tv&plannedPage=2");
  });

  test("慢速翻页保留当前海报、分页焦点和其他分区，只请求目标页", async () => {
    let finish: ((value: Response) => void) | undefined;
    handle = (path) => {
      const url = new URL(path, "http://localhost");
      if (url.pathname !== "/v1/media/shared/entries") return response(path);
      const page = Number(url.searchParams.get("page") ?? 1);
      if (page === 2)
        return new Promise<Response>((resolve) => {
          finish = resolve;
        });
      return json(
        list(
          [{ ...entry, title: `第一页-${url.searchParams.get("status")}` }],
          page,
          48,
        ),
      );
    };
    const host = await render(<App />);
    const shelf = host.querySelector<HTMLElement>(
      '[aria-labelledby="shelf-planned"]',
    );
    if (!shelf) throw new Error("SHELF_MISSING");
    const grid = shelf.querySelector(".poster-grid");
    const next = button(shelf, "下一页");
    await act(async () => next.focus());
    const count = requests.length;
    await click(next);
    expect(window.location.search).toBe("?plannedPage=2");
    expect(shelf.getAttribute("aria-busy")).toBe("true");
    expect(shelf.querySelector(".poster-grid")).toBe(grid);
    expect(grid?.hasAttribute("inert")).toBe(true);
    expect(shelf.textContent).toContain("第一页-planned");
    expect(document.activeElement).toBe(next);
    expect(next.getAttribute("aria-disabled")).toBe("true");
    expect(requests.slice(count)).toHaveLength(1);
    await click(next);
    expect(requests.slice(count)).toHaveLength(1);
    await act(async () =>
      finish?.(json(list([{ ...entry, title: "第二页" }], 2, 48))),
    );
    await settle();
    expect(shelf.getAttribute("aria-busy")).toBe("false");
    expect(grid?.hasAttribute("inert")).toBe(false);
    expect(shelf.textContent).toContain("第二页");
    expect(document.activeElement).toBe(next);
    expect(host.textContent).toContain("第一页-completed");
  });

  test("切换类型立即隐藏旧页，迟到的翻页响应不能覆盖新类型", async () => {
    let finish: ((value: Response) => void) | undefined;
    handle = (path) => {
      const url = new URL(path, "http://localhost");
      if (url.pathname !== "/v1/media/shared/entries") return response(path);
      if (url.searchParams.get("mediaType") === "tv")
        return json(list([{ ...entry, title: "新类型剧集" }], 1, 48));
      if (url.searchParams.get("page") === "2")
        return new Promise<Response>((resolve) => {
          finish = resolve;
        });
      return json(list([{ ...entry, title: "旧类型电影" }], 1, 48));
    };
    const host = await render(<App />);
    const shelf = host.querySelector<HTMLElement>(
      '[aria-labelledby="shelf-planned"]',
    );
    if (!shelf) throw new Error("SHELF_MISSING");
    await click(button(shelf, "下一页"));
    const signal = requests.at(-1)?.init?.signal;
    await click(button(host, "剧集"));
    expect(signal?.aborted).toBe(true);
    expect(host.textContent).not.toContain("旧类型电影");
    await act(async () =>
      finish?.(json(list([{ ...entry, title: "迟到电影" }], 2, 48))),
    );
    await settle();
    expect(host.textContent).toContain("新类型剧集");
    expect(host.textContent).not.toContain("迟到电影");
  });

  test("匿名访问私人地址只显示登录入口，不发私人请求", async () => {
    window.history.replaceState(null, "", "/movies/private");
    const host = await render(<App />);
    expect(host.textContent).toContain("登录后查看自己的私人片库");
    expect(requests.map(({ path }) => path)).toEqual(["/v1/media/session"]);
    expect(
      host.querySelector('a[href="/web/login?next=/movies/"]'),
    ).not.toBeNull();
  });

  test("重新聚焦先隐藏上一账户私库，退出后卸载私人数据", async () => {
    session = { account };
    window.history.replaceState(null, "", "/movies/private");
    const host = await render(<App />);
    expect(host.textContent).toContain("账户甲的私人作品");
    let finish: ((value: Response) => void) | undefined;
    handle = (path) =>
      path === "/v1/media/session"
        ? new Promise((resolve) => {
            finish = resolve;
          })
        : response(path);
    await act(async () => window.dispatchEvent(new Event("focus")));
    const checking = host.querySelector<HTMLElement>("[data-session-content]");
    expect(checking?.hidden).toBe(true);
    expect(checking?.hasAttribute("inert")).toBe(true);
    await act(async () => finish?.(json({ account: null })));
    await settle();
    expect(host.textContent).toContain("登录后查看自己的私人片库");
    expect(host.textContent).not.toContain("账户甲的私人作品");
  });

  test("同账户重新聚焦安全隐藏并恢复类型选择、原视图和控件焦点", async () => {
    session = { account };
    const host = await render(<App />);
    await click(button(host, "电影"));
    const input = button(host, "电影");
    await act(async () => input.focus());
    let finish: ((value: Response) => void) | undefined;
    handle = (path) =>
      path === "/v1/media/session"
        ? new Promise((resolve) => {
            finish = resolve;
          })
        : response(path);
    const reads = requests.filter(({ path }) =>
      path.startsWith("/v1/media/shared/entries"),
    ).length;
    await act(async () => window.dispatchEvent(new Event("focus")));
    expect(
      host.querySelector<HTMLElement>("[data-session-content]")?.hidden,
    ).toBe(true);
    expect(input.isConnected).toBe(true);
    await act(async () => finish?.(json({ account })));
    await settle();
    expect(
      host.querySelector<HTMLElement>("[data-session-content]")?.hidden,
    ).toBe(false);
    expect(button(host, "电影")).toBe(input);
    expect(input.getAttribute("data-state")).toBe("on");
    expect(document.activeElement).toBe(input);
    expect(
      requests.filter(({ path }) =>
        path.startsWith("/v1/media/shared/entries"),
      ),
    ).toHaveLength(reads);
  });

  test("账户切换重新请求私库，不复用上一账户的内容", async () => {
    session = { account };
    window.history.replaceState(null, "", "/movies/private");
    const host = await render(<App />);
    session = {
      account: { ...account, id: "account-b", displayName: "账户乙" },
    };
    handle = (path) =>
      path.startsWith("/v1/media/private/entries")
        ? json(
            list([{ ...entry, scope: "private", title: "账户乙的私人作品" }]),
          )
        : response(path);
    await act(async () => window.dispatchEvent(new Event("focus")));
    await settle();
    expect(host.textContent).toContain("账户乙的私人作品");
    expect(host.textContent).not.toContain("账户甲的私人作品");
    expect(
      requests.filter(({ path }) =>
        path.startsWith("/v1/media/private/entries"),
      ),
    ).toHaveLength(6);
  });

  test("匿名详情显示磁力登录提示，演员作品和来源详情通过真实导航读取", async () => {
    window.history.replaceState(null, "", "/movies/entries/shared/film-1");
    const host = await render(<App />);
    expect(host.textContent).toContain(entry.title);
    expect(host.textContent).toContain("登录后查看磁力链接");
    expect(host.textContent).not.toContain("magnet:?");
    expect(host.textContent).not.toContain("暂无图集");
    expect(host.textContent).not.toContain("还没有观后感");
    expect(host.querySelectorAll("h1")).toHaveLength(1);
    expect(
      host.querySelector('a[href="https://www.themoviedb.org/movie/99"]'),
    ).not.toBeNull();
    await click(button(host, "演员甲"));
    expect(host.textContent).toContain("来源中的作品");
    expect(
      requests.some(({ path }) =>
        path.startsWith("/v1/media/catalog/actors/tmdb/42"),
      ),
    ).toBe(true);
    const link = host.querySelector<HTMLAnchorElement>(
      'a[href="/movies/catalog/tmdb/movie/99"]',
    );
    expect(link).not.toBeNull();
    if (link) await click(link);
    expect(host.textContent).not.toContain(
      "以下为来源资料，不包含私人片库状态。",
    );
    expect(host.textContent).toContain("来源简介");
  });

  test("合集列表、合集详情和条目详情均可读取，ToC 不呈现维护表单", async () => {
    window.history.replaceState(null, "", "/movies/collections");
    const host = await render(<App />);
    const link = host.querySelector<HTMLAnchorElement>(
      'a[href="/movies/collections/collection-1"]',
    );
    expect(link).not.toBeNull();
    if (link) await click(link);
    expect(host.textContent).toContain("周末片单");
    expect(host.textContent).toContain(entry.title);
    expect(host.querySelector("form")).toBeNull();
    expect(requests.every(({ init }) => init?.method === "GET")).toBe(true);
  });

  test("登录后详情复制服务端返回的磁力，页面不显示维护入口", async () => {
    session = { account };
    window.history.replaceState(null, "", "/movies/entries/shared/film-1");
    const writeText = mock(async (_value: string) => {});
    const descriptor = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    let detailReads = 0;
    handle = (path) =>
      path === "/v1/media/shared/entries/film-1"
        ? json({
            item: {
              ...entry,
              magnetUri:
                detailReads++ === 0
                  ? "magnet:?xt=urn:btih:0123456789012345678901234567890123456789"
                  : "magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            },
          })
        : response(path);
    try {
      const host = await render(<App />);
      await click(button(host, "复制磁力链接"));
      expect(writeText).toHaveBeenCalledWith(
        "magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      );
      expect(host.textContent).toContain("已复制");
      expect(host.querySelector("form")).toBeNull();
    } finally {
      if (descriptor) Object.defineProperty(navigator, "clipboard", descriptor);
      else Reflect.deleteProperty(navigator, "clipboard");
    }
  });

  test("私人请求被拒绝时显示账户验证入口，不展示服务端诊断", async () => {
    session = { account };
    window.history.replaceState(null, "", "/movies/private");
    handle = (path) =>
      path.startsWith("/v1/media/private/entries")
        ? json({ code: "FORBIDDEN", detail: "untrusted-sensitive-detail" }, 403)
        : response(path);
    const host = await render(<App />);
    expect(host.textContent).toContain("需要登录或验证邮箱");
    expect(host.querySelector('main a[href="/web/account"]')).not.toBeNull();
    expect(host.textContent).not.toContain("untrusted-sensitive-detail");
  });

  test.each([401, 200])(
    "复制前重新授权：后续详情响应 %s 不再允许磁力时不写剪贴板",
    async (status) => {
      session = { account };
      window.history.replaceState(null, "", "/movies/entries/shared/film-1");
      const writeText = mock(async (_value: string) => {});
      const descriptor = Object.getOwnPropertyDescriptor(
        navigator,
        "clipboard",
      );
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: { writeText },
      });
      let detailReads = 0;
      handle = (path) => {
        if (path !== "/v1/media/shared/entries/film-1") return response(path);
        if (detailReads++ === 0)
          return json({
            item: {
              ...entry,
              magnetUri:
                "magnet:?xt=urn:btih:0123456789012345678901234567890123456789",
            },
          });
        return status === 401
          ? json({ code: "UNAUTHORIZED" }, 401)
          : json({ item: entry });
      };
      try {
        const host = await render(<App />);
        await click(button(host, "复制磁力链接"));
        expect(detailReads).toBe(2);
        expect(writeText).not.toHaveBeenCalled();
        expect(host.querySelector("[role=alert]")).not.toBeNull();
      } finally {
        if (descriptor)
          Object.defineProperty(navigator, "clipboard", descriptor);
        else Reflect.deleteProperty(navigator, "clipboard");
      }
    },
  );

  test.each(["navigation", "session"])(
    "复制请求延迟时 %s 变化中止旧响应写剪贴板",
    async (reason) => {
      session = { account };
      window.history.replaceState(null, "", "/movies/entries/shared/film-1");
      const writeText = mock(async (_value: string) => {});
      const descriptor = Object.getOwnPropertyDescriptor(
        navigator,
        "clipboard",
      );
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: { writeText },
      });
      let detailReads = 0;
      let finish: ((response: Response) => void) | undefined;
      const permitted = {
        item: {
          ...entry,
          magnetUri:
            "magnet:?xt=urn:btih:0123456789012345678901234567890123456789",
        },
      };
      handle = (path) => {
        if (path !== "/v1/media/shared/entries/film-1") return response(path);
        const read = detailReads++;
        if (read === 0) return json(permitted);
        if (read === 1)
          return new Promise((resolve) => {
            finish = resolve;
          });
        return json({ item: entry });
      };
      try {
        const host = await render(<App />);
        await click(button(host, "复制磁力链接"));
        if (reason === "navigation")
          await act(async () => navigate("/movies/"));
        else {
          session = { account: null };
          await act(async () => window.dispatchEvent(new Event("focus")));
        }
        await settle();
        await act(async () => finish?.(json(permitted)));
        await settle();
        expect(detailReads).toBe(reason === "session" ? 3 : 2);
        expect(writeText).not.toHaveBeenCalled();
      } finally {
        if (descriptor)
          Object.defineProperty(navigator, "clipboard", descriptor);
        else Reflect.deleteProperty(navigator, "clipboard");
      }
    },
  );

  test("接口错误只显示固定文案并支持重试，不回显不可信诊断", async () => {
    let attempts = 0;
    handle = (path) =>
      path.startsWith("/v1/media/shared/entries") && attempts++ === 0
        ? json(
            { code: "UNAVAILABLE", detail: "untrusted-sensitive-detail" },
            503,
          )
        : response(path);
    const host = await render(<App />);
    expect(host.textContent).toContain("暂时无法载入");
    expect(host.textContent).not.toContain("untrusted-sensitive-detail");
    await click(button(host, "重试"));
    expect(host.textContent).toContain(entry.title);
  });

  test("英文覆盖导航、全部观看状态、空态和显示偏好，翻译字段与中文一致", async () => {
    setLanguage("en");
    handle = (path) =>
      path.startsWith("/v1/media/shared/entries")
        ? json(list([]))
        : response(path);
    const host = await render(<App />);
    expect(host.textContent).toContain("Shared library");
    expect(host.textContent).toContain("Movies");
    expect(host.textContent).toContain("TV shows");
    expect(host.querySelectorAll(".home-shelf")).toHaveLength(0);
    expect(host.querySelectorAll(".site-empty")).toHaveLength(1);
    expect(host.textContent).toContain("No films in this library yet");
    expect(button(host, "Language and appearance")).toBeDefined();
    expect(Object.keys(mediaSiteResources.en).sort()).toEqual(
      Object.keys(mediaSiteResources["zh-CN"]).sort(),
    );
  });

  test("浏览器返回恢复筛选 URL，非法与跨域路径不进入请求", async () => {
    const host = await render(<App />);
    await act(async () =>
      navigate(wallPath("shared", { mediaType: "tv", page: 1 })),
    );
    await settle();
    expect(button(host, "剧集").getAttribute("data-state")).toBe("on");
    expect(readRoute("/movies/entries/private/%2Fsecret")).toEqual({
      kind: "missing",
    });
    expect(readRoute("/movies/catalog/nope/movie/1")).toEqual({
      kind: "missing",
    });
    expect(readRoute("/admin/")).toEqual({ kind: "missing" });
  });
});
