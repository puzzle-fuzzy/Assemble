import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import {
  browserImageLoader,
  createBrowserMediaClient,
  createBrowserMediaCollectionsClient,
} from "@assemble/media/browser-client";
import type { MediaClient } from "@assemble/media/client";
import { MediaCollectionsManager } from "@assemble/media/components/collections";
import { MediaEditor } from "@assemble/media/components/editor";
import { MediaImport } from "@assemble/media/components/import";
import { MediaDetails } from "@assemble/media/components/media-details";
import { MediaWorkspace } from "@assemble/media/components/workspace";
import type { MediaEntry, MediaJob, MediaList } from "@assemble/media/model";
import { act, useState } from "react";
import {
  chooseSelect,
  selectTrigger,
} from "../../../packages/ui/src/test-support/select";
import { App } from "./App";
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

const entry: MediaEntry = {
  published: true,
  owner: null,
  id: "film-1",
  version: 3,
  scope: "shared",
  mediaType: "movie",
  title: "真实片名",
  originalTitle: "Original",
  year: 2026,
  releaseDate: null,
  overview: "服务端简介",
  genres: ["剧情"],
  episodeCount: null,
  status: "watching",
  watchedAt: null,
  review: "共享观后感",
  notes: "管理员备注",
  magnetUri: "magnet:?xt=urn:btih:0123456789012345678901234567890123456789",
  magnetAvailable: true,
  cast: [],
  identities: [{ provider: "tmdb", mediaType: "movie", externalId: "12" }],
  poster: null,
  createdAt: "2026-11-25T00:00:00Z",
  updatedAt: "2026-11-25T00:00:00Z",
};
const session = {
  account: {
    id: "admin-media",
    displayName: "影视管理员",
    email: "media@example.test",
    avatarUrl: null,
  },
  csrfToken: "media-csrf-test",
};
const capabilities = {
  providers: { tmdb: true, douban: false },
  canMaintainShared: true,
  canMaintainPrivate: true,
};
const candidate = {
  identity: entry.identities[0],
  title: "来源电影",
  originalTitle: "Catalog",
  year: 2025,
  overview: "真实来源资料",
  poster: null,
};
const initialJob: MediaJob = {
  id: "job-1",
  scope: "shared",
  status: "failed",
  stage: "images",
  completedItems: 1,
  totalItems: 2,
  entryId: null,
  errorCode: "SOURCE_UNAVAILABLE",
};
const originalFetch = globalThis.fetch;
const requests: { path: string; init?: RequestInit }[] = [];
let handle: (path: string, init?: RequestInit) => Response | Promise<Response>;
function json(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}
function list(items: MediaEntry[] = [entry], page = 1): MediaList {
  return {
    items,
    total: items.length,
    page,
    pageSize: 20,
    counts: {
      all: items.length,
      planned: 0,
      watching: items.length,
      completed: 0,
    },
  };
}
function response(path: string): Response {
  if (path === "/v1/admin/session") return json(session);
  if (path === "/v1/admin/overview")
    return json({
      users: 1,
      devices: 0,
      onlineDevices: 0,
      registrationsToday: 1,
    });
  if (path === "/v1/media/capabilities") return json(capabilities);
  if (path.startsWith("/v1/admin/media/entries?")) return json(list());
  if (path === `/v1/admin/media/entries/${entry.id}`)
    return json({ item: entry });
  if (path === `/v1/admin/media/entries/${entry.id}/gallery`)
    return json({ entryId: entry.id, version: entry.version, items: [] });
  if (path.startsWith("/v1/media/search?")) return json({ items: [candidate] });
  if (path.startsWith("/v1/media/catalog/details?"))
    return json({ item: candidate });
  if (path.startsWith("/v1/media/collections?"))
    return json({ items: [], total: 0, page: 1, pageSize: 12 });
  return json({}, 404);
}
async function change(
  element: HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement,
  value: string,
) {
  await act(async () => {
    const prototype =
      element instanceof HTMLSelectElement
        ? HTMLSelectElement.prototype
        : element instanceof HTMLTextAreaElement
          ? HTMLTextAreaElement.prototype
          : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, "value")?.set?.call(
      element,
      value,
    );
    element.dispatchEvent(
      new Event(element instanceof HTMLSelectElement ? "change" : "input", {
        bubbles: true,
      }),
    );
  });
  await settle();
}
function field(root: ParentNode, label: string) {
  const found = [...root.querySelectorAll("label")].find(
    (item) => item.textContent === label,
  );
  const control = found?.htmlFor
    ? document.getElementById(found.htmlFor)
    : null;
  if (
    !(
      control instanceof HTMLInputElement ||
      control instanceof HTMLTextAreaElement ||
      control instanceof HTMLSelectElement
    )
  )
    throw new Error(`Missing field ${label}`);
  return control;
}
async function submit(form: HTMLFormElement) {
  await act(async () =>
    form.dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    ),
  );
  await settle();
}
function AccountSurface({ client }: { client: MediaClient }) {
  const [account, setAccount] = useState("first");
  return (
    <>
      <button type="button" onClick={() => setAccount("second")}>
        switch account
      </button>
      <MediaWorkspace
        client={client}
        imageLoader={browserImageLoader}
        canEdit={false}
        accountKey={account}
      />
    </>
  );
}
let previousUrl: string;
beforeEach(() => {
  previousUrl = window.location.href;
  (
    window as unknown as { happyDOM: { setURL: (url: string) => void } }
  ).happyDOM.setURL("http://localhost/admin/");
  setLanguage("zh-CN");
  window.history.replaceState(null, "", "/admin/");
  requests.length = 0;
  handle = response;
  globalThis.fetch = Object.assign(
    mock((input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      requests.push({ path, init });
      return Promise.resolve(handle(path, init));
    }),
    { preconnect: () => {} },
  );
});
afterEach(async () => {
  await cleanup();
  globalThis.fetch = originalFetch;
  (
    window as unknown as { happyDOM: { setURL: (url: string) => void } }
  ).happyDOM.setURL(previousUrl);
  await act(async () => setLanguage("zh-CN"));
});

describe("影视业务界面与权限", () => {
  test("影视编辑下拉只更新草稿，提交仍携带原版本，保存中禁止切换", async () => {
    let finish: ((response: Response) => void) | undefined;
    handle = (path, init) =>
      path === `/v1/admin/media/entries/${entry.id}` && init?.method === "PUT"
        ? new Promise((resolve) => {
            finish = resolve;
          })
        : response(path);
    const client = createBrowserMediaClient("shared", {
      readMode: "admin",
      csrfToken: () => session.csrfToken,
    });
    let saved: MediaEntry | undefined;
    await render(
      <MediaEditor
        client={client}
        entry={entry}
        onClose={() => {}}
        onSaved={(value) => {
          saved = value;
        }}
      />,
    );
    expect(selectTrigger(document.body, "作品类型").textContent).toContain(
      "电影",
    );
    expect(selectTrigger(document.body, "观看状态").textContent).toContain(
      "在看",
    );
    await chooseSelect(document.body, "作品类型", "剧集");
    await chooseSelect(document.body, "观看状态", "想看");
    expect(requests).toHaveLength(0);
    await click(button(document.body, "保存"));
    expect(selectTrigger(document.body, "作品类型").disabled).toBe(true);
    expect(selectTrigger(document.body, "观看状态").disabled).toBe(true);
    expect(JSON.parse(String(requests[0]?.init?.body))).toMatchObject({
      version: entry.version,
      content: { title: entry.title, mediaType: "tv", status: "planned" },
    });
    await act(async () =>
      finish?.(
        json({
          item: { ...entry, mediaType: "tv", status: "planned", version: 4 },
        }),
      ),
    );
    expect(saved).toMatchObject({
      mediaType: "tv",
      status: "planned",
      version: 4,
    });
  });

  test("影视导入下拉保留搜索参数和结果清理，未配置的来源仍禁用", async () => {
    const client = createBrowserMediaClient("shared", { readMode: "admin" });
    await render(
      <MediaImport
        client={client}
        capabilities={{
          ...capabilities,
          providers: { tmdb: true, douban: true },
        }}
        imageLoader={browserImageLoader}
        onClose={() => {}}
        onImported={() => {}}
      />,
    );
    expect(selectTrigger(document.body, "资料源").textContent).toContain(
      "TMDB",
    );
    await chooseSelect(document.body, "作品类型", "剧集");
    expect(requests).toHaveLength(0);
    await change(field(document.body, "片名关键词"), " source title ");
    await click(button(document.body, "搜索"));
    expect(
      new URL(requests.at(-1)?.path ?? "", "http://localhost").searchParams.get(
        "mediaType",
      ),
    ).toBe("tv");
    expect(document.body.textContent).toContain("来源电影");
    const count = requests.length;
    await chooseSelect(document.body, "资料源", "豆瓣");
    expect(requests).toHaveLength(count);
    expect(document.body.textContent).not.toContain("来源电影");
    await click(button(document.body, "搜索"));
    const params = new URL(requests.at(-1)?.path ?? "", "http://localhost")
      .searchParams;
    expect(Object.fromEntries(params)).toMatchObject({
      provider: "douban",
      mediaType: "tv",
      q: "source title",
    });
    await cleanup();
    await render(
      <MediaImport
        client={client}
        capabilities={capabilities}
        imageLoader={browserImageLoader}
        onClose={() => {}}
        onImported={() => {}}
      />,
    );
    await act(async () =>
      selectTrigger(document.body, "资料源").dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
      ),
    );
    await waitFor(
      () => document.querySelector('[role="option"]') !== null,
      "来源选项",
    );
    const disabled = [
      ...document.querySelectorAll<HTMLElement>('[role="option"]'),
    ].find((item) => item.textContent?.trim() === "豆瓣");
    expect(disabled?.getAttribute("aria-disabled")).toBe("true");
  });

  test("管理端不可代公开，已公开记录可撤下或在保存前恢复原选择", async () => {
    const owned: MediaEntry = {
      ...entry,
      scope: "private",
      owner: { id: "owner-1", displayName: "他人", avatarUrl: null },
      published: false,
    };
    handle = (path) =>
      path.startsWith("/v1/admin/media/entries?")
        ? json(list([owned]))
        : path === `/v1/admin/media/entries/${owned.id}`
          ? json({ item: owned })
          : response(path);
    const host = await render(<App />);
    await navigate(host, "media");
    await click(button(host, `查看 ${owned.title} 的详情`));
    await click(button(document.body, "编辑"));
    let checkbox = document.body.querySelector<HTMLButtonElement>(
      '[role="checkbox"][id$="-published"]',
    );
    if (!checkbox) throw new Error("missing publication choice");
    expect(checkbox.disabled).toBe(true);
    expect(checkbox.getAttribute("aria-checked")).toBe("false");
    expect(document.body.textContent).toContain("只有所有者本人能公开这条记录");
    await act(async () => setLanguage("en"));
    expect(document.body.textContent).toContain(
      "Only the owner can publish this entry.",
    );
    await act(async () => setLanguage("zh-CN"));
    await cleanup();
    requests.length = 0;
    const client = createBrowserMediaClient("shared", {
      readMode: "admin",
      csrfToken: () => "media-csrf-test",
    });
    handle = (_path, init) =>
      json({
        item: {
          ...owned,
          published: JSON.parse(String(init?.body)).content.published,
          version: owned.version + 1,
        },
      });
    const saved = mock((value: MediaEntry) => value);
    await render(
      <MediaEditor
        client={client}
        entry={{ ...owned, published: true }}
        viewerAccountId={session.account.id}
        onClose={() => {}}
        onSaved={saved}
      />,
    );
    checkbox = document.body.querySelector<HTMLButtonElement>(
      '[role="checkbox"][id$="-published"]',
    );
    if (!checkbox) throw new Error("missing publication choice");
    expect(checkbox.disabled).toBe(false);
    await click(checkbox);
    expect(checkbox.getAttribute("aria-checked")).toBe("false");
    expect(checkbox.disabled).toBe(false);
    await click(checkbox);
    expect(checkbox.getAttribute("aria-checked")).toBe("true");
    await click(button(document.body, "保存"));
    expect(JSON.parse(String(requests.at(-1)?.init?.body))).toMatchObject({
      content: { published: true },
    });
    await cleanup();
    await render(
      <MediaEditor
        client={client}
        entry={{ ...owned, published: true }}
        viewerAccountId={session.account.id}
        onClose={() => {}}
        onSaved={saved}
      />,
    );
    checkbox = document.body.querySelector<HTMLButtonElement>(
      '[role="checkbox"][id$="-published"]',
    );
    if (!checkbox) throw new Error("missing publication choice");
    await click(checkbox);
    await click(button(document.body, "保存"));
    expect(JSON.parse(String(requests.at(-1)?.init?.body))).toMatchObject({
      content: { published: false },
    });
    await cleanup();
    await render(
      <MediaEditor
        client={client}
        entry={{
          ...owned,
          owner: {
            id: session.account.id,
            displayName: "管理员本人",
            avatarUrl: null,
          },
        }}
        viewerAccountId={session.account.id}
        onClose={() => {}}
        onSaved={saved}
      />,
    );
    checkbox = document.body.querySelector<HTMLButtonElement>(
      '[role="checkbox"][id$="-published"]',
    );
    if (!checkbox) throw new Error("missing own publication choice");
    expect(checkbox.disabled).toBe(false);
    await click(checkbox);
    expect(checkbox.getAttribute("aria-checked")).toBe("true");
    expect(host).toBeTruthy();
  });

  test("管理详情复用封面预览和轮播，关闭原图保留详情、筛选与维护操作", async () => {
    const poster = {
      assetId: "admin-cover",
      width: 400,
      height: 600,
      contentType: "image/webp",
    };
    const second = { ...poster, assetId: "admin-second" };
    handle = (path) =>
      path === `/v1/admin/media/entries/${entry.id}`
        ? json({ item: { ...entry, poster } })
        : path.endsWith(`/${entry.id}/gallery`)
          ? json({
              entryId: entry.id,
              version: entry.version,
              items: [poster, second],
            })
          : response(path);
    const host = await render(<App />);
    await navigate(host, "media");
    await search(host, "真实");
    await click(button(host, "查看 真实片名 的详情"));
    await waitForText(document.body, "浏览海报（2）");
    const details = document.body.querySelector<HTMLElement>('[role="dialog"]');
    if (!details) throw new Error("DETAILS_NOT_OPEN");
    expect(
      details.querySelector('[data-presentation="management"]'),
    ).not.toBeNull();
    const cover = button(details, "放大《真实片名》海报");
    await click(cover);
    await waitFor(
      () => document.querySelectorAll('[role="dialog"]').length === 2,
      "嵌套原图预览",
    );
    const preview = document.querySelector<HTMLElement>(".detail-lightbox");
    if (!preview) throw new Error("PREVIEW_NOT_OPEN");
    await waitFor(
      () =>
        preview
          .querySelector("img")
          ?.getAttribute("src")
          ?.endsWith("variant=original") === true,
      "原图加载",
    );
    await click(button(preview, "下一张"));
    await waitFor(
      () =>
        preview
          .querySelector("img.detail-lightbox-image")
          ?.getAttribute("src")
          ?.includes(second.assetId) === true,
      "轮播第二张",
    );
    await act(async () =>
      preview.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      ),
    );
    await waitFor(
      () => document.querySelector(".detail-lightbox") === null,
      "关闭原图",
    );
    await waitFor(() => document.activeElement === cover, "焦点返回详情封面");
    expect(details.isConnected).toBe(true);
    expect(button(details, "编辑")).toBeDefined();
    expect(button(details, "图集与封面")).toBeDefined();
    expect(button(details, "删除")).toBeDefined();
    expect(
      host.querySelector<HTMLInputElement>('input[type="search"]')?.value,
    ).toBe("真实");
    expect(
      requests.filter(({ init }) => init?.method && init.method !== "GET"),
    ).toHaveLength(0);
  });

  test("管理封面解码失败后能重试，失败占位不被放大按钮遮挡", async () => {
    const poster = {
      assetId: "admin-broken-cover",
      width: 400,
      height: 600,
      contentType: "image/webp",
    };
    handle = (path) =>
      path === `/v1/admin/media/entries/${entry.id}`
        ? json({ item: { ...entry, poster } })
        : response(path);
    const host = await render(<App />);
    await navigate(host, "media");
    await click(button(host, "查看 真实片名 的详情"));
    await waitFor(
      () => Boolean(document.querySelector(".detail-cover img")),
      "封面元素",
    );
    const frame = document.querySelector(".detail-cover");
    const image = frame?.querySelector("img");
    if (!image) throw new Error("COVER_NOT_FOUND");
    await act(async () => image.dispatchEvent(new Event("error")));
    const retry = button(document.body, "重新加载图片：真实片名");
    expect(document.querySelector(".detail-cover")).toBe(frame);
    expect(frame?.getAttribute("data-image-state")).toBe("failed");
    expect(document.querySelector(".detail-poster-trigger")).toBeNull();
    await click(retry);
    await waitFor(() => Boolean(frame?.querySelector("img")), "重新加载封面");
    expect(frame?.querySelector("img")).not.toBe(image);
    expect(button(document.body, "放大《真实片名》海报")).toBeDefined();
    expect(document.querySelectorAll('[role="dialog"]').length).toBe(1);
  });

  test("管理预览期间图集请求发现会话失效，卸载详情与原图，不留下旧账户图片", async () => {
    const poster = {
      assetId: "admin-auth-cover",
      width: 400,
      height: 600,
      contentType: "image/webp",
    };
    let deny: ((value: Response) => void) | undefined;
    handle = (path) =>
      path === `/v1/admin/media/entries/${entry.id}`
        ? json({ item: { ...entry, poster } })
        : path.endsWith(`/${entry.id}/gallery`)
          ? new Promise<Response>((resolve) => {
              deny = resolve;
            })
          : response(path);
    const host = await render(<App />);
    await navigate(host, "media");
    await click(button(host, "查看 真实片名 的详情"));
    await waitFor(
      () => Boolean(document.querySelector(".detail-cover img")),
      "封面元素",
    );
    await click(button(document.body, "放大《真实片名》海报"));
    await waitFor(
      () => Boolean(document.querySelector(".detail-lightbox")),
      "原图预览",
    );
    await act(async () => deny?.(json({ code: "UNAUTHORIZED" }, 401)));
    await waitFor(
      () => document.querySelectorAll('[role="dialog"]').length === 0,
      "失效后清除弹窗",
    );
    expect(document.querySelector('[data-poster-motion="true"]')).toBeNull();
    expect(document.body.textContent).not.toContain(entry.notes);
  });

  test("私人记录默认不公开，发布和撤销都必须明确保存，内容携带当前版本", async () => {
    const owned: MediaEntry = {
      ...entry,
      scope: "private",
      owner: { id: "owner-1", displayName: "本人", avatarUrl: null },
      published: false,
    };
    const client = createBrowserMediaClient("private", {
      csrfToken: () => "media-csrf-test",
    });
    handle = (path, init) =>
      path === `/v1/media/private/entries/${owned.id}` && init?.method === "PUT"
        ? json({
            item: {
              ...owned,
              ...JSON.parse(String(init.body)).content,
              version: owned.version + 1,
            },
          })
        : response(path);
    const saved = mock((value: MediaEntry) => value);
    let host = await render(
      <MediaEditor
        client={client}
        entry={owned}
        viewerAccountId="owner-1"
        onClose={() => {}}
        onSaved={saved}
      />,
    );
    let checkbox = document.body.querySelector<HTMLButtonElement>(
      '[role="checkbox"][id$="-published"]',
    );
    if (!checkbox) throw new Error("missing publication choice");
    expect(checkbox.getAttribute("aria-checked")).toBe("false");
    await click(checkbox);
    expect(requests.filter((item) => item.init?.method === "PUT")).toHaveLength(
      0,
    );
    await click(button(document.body, "保存"));
    expect(JSON.parse(String(requests.at(-1)?.init?.body))).toMatchObject({
      version: owned.version,
      content: { published: true },
    });
    expect(saved.mock.calls[0]?.[0].published).toBe(true);
    await cleanup();
    requests.length = 0;
    host = await render(
      <MediaEditor
        client={client}
        entry={{ ...owned, published: true, version: owned.version + 1 }}
        viewerAccountId="owner-1"
        onClose={() => {}}
        onSaved={saved}
      />,
    );
    checkbox = document.body.querySelector<HTMLButtonElement>(
      '[role="checkbox"][id$="-published"]',
    );
    if (!checkbox) throw new Error("missing publication choice");
    expect(checkbox.getAttribute("aria-checked")).toBe("true");
    await click(checkbox);
    await click(button(document.body, "保存"));
    expect(JSON.parse(String(requests.at(-1)?.init?.body))).toMatchObject({
      version: owned.version + 1,
      content: { published: false },
    });
    expect(host).toBeTruthy();
  });
  test("管理影视详情保留查询和列表，关闭弹窗后焦点回到原行", async () => {
    const host = await render(<App />);
    await navigate(host, "media");
    await search(host, "真实");
    const publicPage =
      host.querySelector<HTMLAnchorElement>('a[href="/movies/"]');
    expect(publicPage?.textContent).toBe("查看影视站");
    expect(publicPage?.target).toBe("_blank");
    expect(publicPage?.rel).toContain("noopener");
    expect(publicPage?.getAttribute("aria-description")).toBe("在新标签页打开");
    expect(
      publicPage?.closest('[data-slot="tabs-content"]')?.querySelector("table"),
    ).not.toBeNull();
    const trigger = button(host, "查看 真实片名 的详情");
    trigger.focus();
    await click(trigger);
    await waitForText(document.body, entry.overview);
    const dialog = document.body.querySelector<HTMLElement>('[role="dialog"]');
    expect(dialog?.textContent).toContain(entry.overview);
    expect(host.querySelector("tbody")?.textContent).toContain(entry.title);
    expect(
      host.querySelector<HTMLInputElement>('input[type="search"]')?.value,
    ).toBe("真实");
    expect(
      new URLSearchParams(window.location.hash.split("?")[1]).get("q"),
    ).toBe("真实");
    expect(host.querySelector('thead [role="checkbox"]')).not.toBeNull();
    expect(host.querySelector('th[data-table-column="drag"]')).not.toBeNull();
    await click(button(document.body, "关闭"));
    expect(document.body.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement === trigger).toBe(true);
    const requestCount = requests.length;
    await act(async () => setLanguage("en"));
    expect(publicPage?.textContent).toBe("View media site");
    expect(publicPage?.getAttribute("aria-description")).toBe(
      "Opens in a new tab",
    );
    expect(requests).toHaveLength(requestCount);
    expect(
      host.querySelector<HTMLInputElement>('input[type="search"]')?.value,
    ).toBe("真实");
  });

  test("管理合集使用表格和服务端分页，详情弹窗读取完整成员并保留列表", async () => {
    const collection = {
      id: "collection-table",
      version: 1,
      title: "太空系列",
      description: "合集说明文字",
      poster: null,
      items: [entry],
      createdAt: entry.createdAt,
      updatedAt: entry.updatedAt,
    };
    handle = (path) =>
      path.startsWith("/v1/media/collections?")
        ? json({
            items: [collection],
            total: 23,
            page: Number(
              new URL(path, "http://localhost").searchParams.get("page"),
            ),
            pageSize: Number(
              new URL(path, "http://localhost").searchParams.get("pageSize") ??
                20,
            ),
          })
        : path === `/v1/media/collections/${collection.id}`
          ? json({ item: collection })
          : response(path);
    const host = await render(<App />);
    await navigate(host, "media");
    await act(async () => {
      const tab = button(host, "影视合集");
      tab.focus();
      tab.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
      );
    });
    await waitForText(host, collection.title);
    const publicPage = host.querySelector<HTMLAnchorElement>(
      'a[href="/movies/collections"]',
    );
    expect(publicPage?.textContent).toBe("查看合集页");
    expect(publicPage?.target).toBe("_blank");
    expect(host.querySelector('a[href="/movies/"]')).toBeNull();
    expect(host.querySelector("thead")?.textContent).toContain("更新时间");
    await click(button(host, "下一页"));
    expect(requests.at(-1)?.path).toContain("page=2");
    const trigger = button(host, "查看详情：太空系列");
    await act(async () => trigger.focus());
    await click(trigger);
    await waitForText(document.body, entry.title);
    const dialog = document.body.querySelector('[role="dialog"]');
    expect(dialog?.textContent).toContain(collection.description);
    expect(dialog?.textContent).toContain(entry.title);
    expect(host.querySelector("tbody")?.textContent).toContain(
      collection.title,
    );
    await click(button(document.body, "关闭"));
    expect(host.textContent).toContain("第 2 / 2 页");
    expect(document.activeElement === trigger).toBe(true);
    await act(async () => setLanguage("en"));
    expect(publicPage?.textContent).toBe("View collections");
  });
  test("筛选位于表头，搜索仅用占位文字，输入框清空图标重置全部条件并保留焦点和分页条数", async () => {
    const host = await render(
      <MediaWorkspace
        client={createBrowserMediaClient("shared", { readMode: "admin" })}
        imageLoader={browserImageLoader}
        canEdit={false}
        accountKey="filters"
      />,
    );
    const form = host.querySelector<HTMLFormElement>(
      'form[aria-label="搜索片库"]',
    );
    if (!form) throw new Error("Library filters missing");
    expect(
      [...form.querySelectorAll("label")].map((label) => label.textContent),
    ).toEqual(["搜索片库"]);
    expect(form.querySelector("label")?.classList.contains("sr-only")).toBe(
      true,
    );
    expect(form.querySelectorAll('[role="combobox"]')).toHaveLength(0);
    expect(host.textContent).not.toContain("共享条目由管理员维护");
    const trigger = button(host, "作品类型: 全部类型");
    expect(trigger.closest("th")).not.toBeNull();
    expect(button(host, "观看状态: 全部状态").closest("th")).not.toBeNull();
    await act(async () => {
      trigger.focus();
      trigger.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
      );
    });
    await settle();
    const option = [
      ...document.querySelectorAll<HTMLElement>('[role="menuitemradio"]'),
    ].find((item) => item.textContent === "剧集");
    if (!option) throw new Error("TV option missing");
    await click(option);
    await act(async () => {
      const status = button(host, "观看状态: 全部状态");
      status.focus();
      status.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
      );
    });
    await settle();
    const watching = [
      ...document.querySelectorAll<HTMLElement>('[role="menuitemradio"]'),
    ].find((item) => item.textContent === "在看");
    if (!watching) throw new Error("Viewing status option missing");
    await click(watching);
    await change(field(host, "搜索片库"), " 星球 ");
    await click(button(host, "搜索"));
    const path = [...requests]
      .reverse()
      .find((request) => request.path.includes("/entries?"))?.path;
    const query = new URL(path ?? "", "http://localhost").searchParams;
    expect(query.get("mediaType")).toBe("tv");
    expect(query.get("status")).toBe("watching");
    expect(query.get("q")).toBe("星球");
    expect(query.get("page")).toBe("1");
    expect(query.has("genre")).toBe(false);
    expect(query.has("year")).toBe(false);
    const requestCount = requests.length;
    await act(async () => setLanguage("en"));
    expect(requests).toHaveLength(requestCount);
    expect(button(host, "Type: TV series").closest("th")).not.toBeNull();
    expect(
      button(host, "Viewing status: Watching").closest("th"),
    ).not.toBeNull();
    const clear = button(host, "Clear filters");
    expect(
      clear
        .closest('[data-slot="input-group-addon"]')
        ?.getAttribute("data-align"),
    ).toBe("inline-end");
    expect(clear.textContent).toBe("");
    await click(clear);
    expect(document.activeElement).toBe(
      host.querySelector('input[type="search"]'),
    );
    expect(
      (host.querySelector('input[type="search"]') as HTMLInputElement).value,
    ).toBe("");
    const cleared = [...requests]
      .reverse()
      .find((request) => request.path.includes("/entries?"))?.path;
    const reset = new URL(cleared ?? "", "http://localhost").searchParams;
    expect(reset.has("mediaType")).toBe(false);
    expect(reset.has("status")).toBe(false);
    expect(reset.has("q")).toBe(false);
    expect(reset.get("pageSize")).toBe(query.get("pageSize"));
  });

  test("空表格仅显示本地化文字，不显示空态图标", async () => {
    handle = (path) =>
      path.startsWith("/v1/admin/media/entries?")
        ? json(list([]))
        : response(path);
    const host = await render(
      <MediaWorkspace
        client={createBrowserMediaClient("shared", { readMode: "admin" })}
        imageLoader={browserImageLoader}
        canEdit={false}
        accountKey="empty"
      />,
    );
    const body = host.querySelector("tbody");
    expect(body?.textContent).toBe("还没有条目");
    expect(body?.querySelector("svg")).toBeNull();
    await act(async () => setLanguage("en"));
    expect(body?.textContent).toBe("No entries yet");
    expect(body?.querySelector("svg")).toBeNull();
  });

  test("详情中的刷新返回最新列表，不保留已过期的详情版本", async () => {
    const client = createBrowserMediaClient("shared", { readMode: "admin" });
    const host = await render(
      <MediaWorkspace
        client={client}
        imageLoader={browserImageLoader}
        canEdit={false}
        accountKey="admin-media"
      />,
    );
    await click(button(host, "查看 真实片名 的详情"));
    expect(host.textContent).toContain("服务端简介");
    handle = (path) =>
      path.startsWith("/v1/admin/media/entries?")
        ? json(list([{ ...entry, title: "更新后片名", version: 4 }]))
        : response(path);
    await click(button(host, "刷新"));
    expect(host.textContent).not.toContain("服务端简介");
    expect(host.textContent).toContain("更新后片名");
  });
  test("管理入口读取真实片库，携带版本和CSRF更新三种状态；冲突保留输入", async () => {
    let conflict = true;
    handle = (path, init) =>
      !conflict && path.startsWith("/v1/admin/media/entries?")
        ? json(list([{ ...entry, version: 4, status: "completed" }]))
        : path === `/v1/admin/media/entries/${entry.id}` &&
            init?.method === "PUT"
          ? conflict
            ? json({ code: "VERSION_CONFLICT" }, 409)
            : json({ item: { ...entry, version: 4, status: "completed" } })
          : response(path);
    const host = await render(<App />);
    await navigate(host, "media");
    await waitForText(host, "真实片名");
    expect(document.title).toContain("影视片库");
    await click(button(host, "查看 真实片名 的详情"));
    await click(button(document.body, "编辑"));
    const editor = [...document.body.querySelectorAll('[role="dialog"]')].at(
      -1,
    );
    if (!editor) throw new Error("missing editor");
    await chooseSelect(editor, "观看状态", "已看");
    await change(field(document.body, "片名"), "保留我的输入");
    await click(button(document.body, "保存"));
    const update = requests.find((item) => item.init?.method === "PUT");
    expect(JSON.parse(String(update?.init?.body))).toMatchObject({
      version: 3,
      content: { title: "保留我的输入", status: "completed" },
    });
    expect(update?.init?.credentials).toBe("same-origin");
    expect(update?.init?.headers).toMatchObject({
      "X-CSRF-Token": "media-csrf-test",
    });
    expect(field(document.body, "片名").value).toBe("保留我的输入");
    expect(button(document.body, "保存").disabled).toBe(true);
    conflict = false;
    await click(button(document.body, "载入最新资料"));
    expect(field(document.body, "片名").value).toBe(entry.title);
    await chooseSelect(editor, "观看状态", "已看");
    await click(button(document.body, "保存"));
    expect(document.body.querySelector('[role="dialog"]')).toBeNull();
    expect(host.textContent).toContain("已看");
  });
  test("失效会话清除管理内容，来源错误正文不进入页面", async () => {
    const host = await render(<App />);
    await navigate(host, "media");
    await waitForText(host, entry.title);
    handle = (path) =>
      path.startsWith("/v1/admin/media/entries?")
        ? json({ code: "HTTP_401", message: "secret diagnostics" }, 401)
        : response(path);
    await click(button(host, "刷新"));
    expect(host.textContent).toContain("登录管理控制台");
    expect(host.textContent).not.toContain(entry.title);
    expect(host.textContent).not.toContain("secret diagnostics");
  });
  test("只读详情复制交给具名适配，不在DOM输出磁力值；匿名提示登录", async () => {
    const copy = mock(async (_entry: MediaEntry) => {});
    const host = await render(
      <MediaDetails
        entry={entry}
        imageLoader={browserImageLoader}
        onCopyMagnet={copy}
      />,
    );
    expect(host.innerHTML).not.toContain("magnet:?");
    await click(button(host, "复制磁力链接"));
    expect(copy).toHaveBeenCalledWith(entry);
    expect(host.textContent).toContain("已复制");
    await cleanup();
    const signIn = mock(() => {});
    const anonymous = await render(
      <MediaDetails
        entry={{ ...entry, magnetUri: null, notes: null }}
        imageLoader={browserImageLoader}
        onSignIn={signIn}
      />,
    );
    await click(button(anonymous, "登录后查看磁力链接"));
    expect(signIn).toHaveBeenCalledTimes(1);
    expect(anonymous.textContent).not.toContain("管理员备注");
  });
  test("管理页面卸载后，迟到的复制授权响应不能写入剪贴板", async () => {
    const descriptor = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const writeText = mock(async (_value: string) => {});
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    try {
      const host = await render(<App />);
      await navigate(host, "media");
      await click(button(host, "查看 真实片名 的详情"));
      let resolve: ((response: Response) => void) | undefined;
      let signal: AbortSignal | null | undefined;
      handle = (path, init) =>
        path === `/v1/admin/media/entries/${entry.id}`
          ? new Promise((done) => {
              resolve = done;
              signal = init?.signal;
            })
          : response(path);
      await click(button(document.body, "复制磁力链接"));
      expect(signal?.aborted).toBe(false);
      await cleanup();
      expect(signal?.aborted).toBe(true);
      await act(async () => resolve?.(json({ item: entry })));
      await settle();
      expect(writeText).not.toHaveBeenCalled();
    } finally {
      if (descriptor) Object.defineProperty(navigator, "clipboard", descriptor);
      else Reflect.deleteProperty(navigator, "clipboard");
    }
  });
  test("查询变化和账户切换取消慢详情，迟到响应不能恢复旧内容", async () => {
    let resolve: ((response: Response) => void) | undefined;
    let signal: AbortSignal | null | undefined;
    handle = (path, init) =>
      path === `/v1/admin/media/entries/${entry.id}`
        ? new Promise((done) => {
            resolve = done;
            signal = init?.signal;
          })
        : response(path);
    const client = createBrowserMediaClient("shared", { readMode: "admin" });
    const host = await render(<AccountSurface client={client} />);
    await click(button(host, "查看 真实片名 的详情"));
    expect(signal?.aborted).toBe(false);
    await search(host, "new query");
    expect(signal?.aborted).toBe(true);
    await act(async () => resolve?.(json({ item: entry })));
    await settle();
    expect(host.textContent).not.toContain("服务端简介");
    await click(button(host, "查看 真实片名 的详情"));
    await click(button(host, "switch account"));
    expect(signal?.aborted).toBe(true);
    await act(async () => resolve?.(json({ item: entry })));
    await settle();
    expect(host.textContent).not.toContain("管理员备注");
    expect(host.textContent).not.toContain("手动添加");
  });
  test("来源核对和导入展示真实阶段，已失败任务重试使用新请求id", async () => {
    let imported = false;
    handle = (path, init) =>
      path === "/v1/admin/media/imports" && init?.method === "POST"
        ? json(
            imported
              ? {
                  ...initialJob,
                  status: "completed",
                  entryId: entry.id,
                  errorCode: null,
                }
              : initialJob,
          )
        : response(path);
    const host = await render(<App />);
    await navigate(host, "media");
    await click(button(host, "来源搜索导入"));
    await change(field(document.body, "片名关键词"), "source title");
    const dialog = document.body.querySelector<HTMLElement>('[role="dialog"]');
    if (!dialog) throw new Error("missing dialog");
    await submit(dialog.querySelector("form") as HTMLFormElement);
    expect(dialog.textContent).toContain("来源电影");
    await click(button(dialog, "核对资料"));
    await click(button(dialog, "导入当前片库"));
    expect(dialog.textContent).toContain("下载并校验图片");
    expect(dialog.textContent).toContain("已处理 1 / 2 项");
    expect(dialog.textContent).toContain("任务失败");
    imported = true;
    await click(button(dialog, "重试任务"));
    const imports = requests.filter(
      (item) => item.path === "/v1/admin/media/imports",
    );
    expect(imports).toHaveLength(2);
    expect(JSON.parse(String(imports[0].init?.body)).requestId).not.toBe(
      JSON.parse(String(imports[1].init?.body)).requestId,
    );
    expect(host.textContent).toContain(entry.title);
    expect(document.body.querySelector('[role="dialog"]')).toBeNull();
  });
  test("合集从共享库选择并维护实际顺序，保存只提交成员标识", async () => {
    const second = { ...entry, id: "film-2", title: "第二部" };
    handle = (path, init) =>
      path.startsWith("/v1/admin/media/entries?")
        ? json(list([entry, second]))
        : path === "/v1/admin/media/collections" && init?.method === "POST"
          ? json({
              item: {
                id: "collection-1",
                version: 1,
                title: "新合集",
                description: "",
                poster: null,
                items: [second, entry],
                createdAt: entry.createdAt,
                updatedAt: entry.updatedAt,
              },
            })
          : response(path);
    const options = { csrfToken: () => session.csrfToken };
    const host = await render(
      <MediaCollectionsManager
        client={createBrowserMediaCollectionsClient(options)}
        libraryClient={createBrowserMediaClient("shared", {
          ...options,
          readMode: "admin",
        })}
        imageLoader={browserImageLoader}
        accountKey="admin"
      />,
    );
    await click(button(host, "新建合集"));
    const dialog = document.body.querySelector<HTMLElement>('[role="dialog"]');
    if (!dialog) throw new Error("missing collection dialog");
    await change(field(document.body, "合集名称"), "新合集");
    const forms = dialog.querySelectorAll<HTMLFormElement>("form");
    await submit(forms[1]);
    const add = [
      ...dialog.querySelectorAll<HTMLButtonElement>("button"),
    ].filter((item) => item.textContent === "添加");
    await click(add[0]);
    await click(add[1]);
    const dragHandle = button(dialog, "调整 第二部 的顺序");
    if (!dragHandle) throw new Error("missing collection member drag handle");
    await act(async () => {
      dragHandle.focus();
      dragHandle.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "ArrowUp",
          altKey: true,
          bubbles: true,
        }),
      );
    });
    expect(
      [
        ...dialog.querySelectorAll<HTMLTableRowElement>(
          "tbody tr[data-table-row]",
        ),
      ].map((row) => row.dataset.tableRow),
    ).toEqual(["film-2", "film-1"]);
    expect(document.activeElement).toBe(dragHandle);
    await click(button(dialog, "下移：第二部"));
    await click(button(dialog, "上移：第二部"));
    await submit(forms[0]);
    const save = requests.find(
      (item) =>
        item.path === "/v1/admin/media/collections" &&
        item.init?.method === "POST",
    );
    expect(JSON.parse(String(save?.init?.body))).toMatchObject({
      title: "新合集",
      entryIds: ["film-2", "film-1"],
    });
    expect(String(save?.init?.body)).not.toContain("magnet");
  });
  test("所有统计表保留两列，汇总数据的拖拽和多选同时禁用", async () => {
    handle = (path) =>
      path === "/v1/admin/media/statistics"
        ? json({
            total: 2,
            planned: 1,
            watching: 1,
            completed: 0,
            movies: 2,
            tv: 0,
            byYear: [{ year: 2026, count: 2 }],
            byGenre: [{ genre: "剧情", count: 2 }],
            byMonth: [],
          })
        : response(path);
    const host = await render(<App />);
    await navigate(host, "media");
    await act(async () => {
      const tab = button(host, "全站片库统计");
      tab.focus();
      tab.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
      );
    });
    await waitForText(host, "类型分布");
    expect(host.querySelectorAll("table")).toHaveLength(3);
    for (const table of host.querySelectorAll("table")) {
      expect(
        table.querySelector('th[data-table-column="drag"]'),
      ).not.toBeNull();
      expect(
        table.querySelector('th[data-table-column="select"]'),
      ).not.toBeNull();
      for (const control of table.querySelectorAll<HTMLButtonElement>(
        '[role="checkbox"], [data-table-drag-handle]',
      ))
        expect(control.disabled).toBe(true);
    }
  });
  test("关闭导入窗口停止轮询，重新打开能续看原任务且不重复入队", async () => {
    handle = (path, init) =>
      path === "/v1/admin/media/imports" && init?.method === "POST"
        ? json({
            ...initialJob,
            status: "queued",
            stage: null,
            completedItems: null,
            totalItems: null,
            errorCode: null,
          })
        : path === "/v1/admin/media/jobs/job-1"
          ? json({
              ...initialJob,
              status: "completed",
              entryId: entry.id,
              errorCode: null,
            })
          : response(path);
    const host = await render(<App />);
    await navigate(host, "media");
    await click(button(host, "来源搜索导入"));
    await change(field(document.body, "片名关键词"), "source title");
    let dialog = document.body.querySelector<HTMLElement>('[role="dialog"]');
    if (!dialog) throw new Error("missing dialog");
    await submit(dialog.querySelector("form") as HTMLFormElement);
    await click(button(dialog, "核对资料"));
    await click(button(dialog, "导入当前片库"));
    expect(dialog.textContent).toContain("正在排队");
    expect(dialog.querySelector('[role="progressbar"]')).toBeNull();
    const close = dialog.querySelector<HTMLButtonElement>(
      'button[data-slot="dialog-close"]',
    );
    if (!close) throw new Error("missing close");
    await click(close);
    expect(document.body.querySelector('[role="dialog"]')).toBeNull();
    await click(button(host, "来源搜索导入"));
    dialog = document.body.querySelector<HTMLElement>('[role="dialog"]');
    if (!dialog) throw new Error("missing reopened dialog");
    await click(button(dialog, "继续查看任务"));
    await act(async () => new Promise((resolve) => setTimeout(resolve, 1250)));
    await settle();
    expect(
      requests.filter((item) => item.path === "/v1/admin/media/imports"),
    ).toHaveLength(1);
    expect(
      requests.some((item) => item.path === "/v1/admin/media/jobs/job-1"),
    ).toBe(true);
    expect(host.textContent).toContain(entry.title);
    expect(document.body.querySelector('[role="dialog"]')).toBeNull();
  });
  test("导入提交响应丢失时重试保留原请求id，避免未知结果重复创建", async () => {
    let first = true;
    handle = (path, init) =>
      path === "/v1/admin/media/imports" && init?.method === "POST"
        ? first
          ? json({ code: "TEMPORARILY_UNAVAILABLE" }, 503)
          : json({
              ...initialJob,
              status: "completed",
              entryId: entry.id,
              errorCode: null,
            })
        : response(path);
    const host = await render(<App />);
    await navigate(host, "media");
    await click(button(host, "来源搜索导入"));
    await change(field(document.body, "片名关键词"), "source title");
    const dialog = document.body.querySelector<HTMLElement>('[role="dialog"]');
    if (!dialog) throw new Error("missing dialog");
    await submit(dialog.querySelector("form") as HTMLFormElement);
    await click(button(dialog, "核对资料"));
    await click(button(dialog, "导入当前片库"));
    first = false;
    await click(button(dialog, "重试任务"));
    const attempts = requests.filter(
      (item) => item.path === "/v1/admin/media/imports",
    );
    expect(attempts).toHaveLength(2);
    expect(JSON.parse(String(attempts[0].init?.body)).requestId).toBe(
      JSON.parse(String(attempts[1].init?.body)).requestId,
    );
  });
  test("图集封面选择提交资产id和最新版本，不提交远程地址", async () => {
    const image = {
      assetId: "asset-public",
      width: 600,
      height: 900,
      contentType: "image/jpeg",
    };
    handle = (path, init) =>
      path === `/v1/admin/media/entries/${entry.id}/gallery`
        ? json({ entryId: entry.id, version: 3, items: [image] })
        : path === `/v1/admin/media/entries/${entry.id}/poster` &&
            init?.method === "PATCH"
          ? json({ item: { ...entry, version: 4, poster: image } })
          : response(path);
    const host = await render(<App />);
    await navigate(host, "media");
    await click(button(host, "查看 真实片名 的详情"));
    await click(button(document.body, "图集与封面"));
    const dialog = [
      ...document.body.querySelectorAll<HTMLElement>('[role="dialog"]'),
    ].at(-1);
    if (!dialog) throw new Error("missing gallery");
    await click(button(dialog, "设为封面"));
    const patch = requests.find((item) => item.init?.method === "PATCH");
    expect(JSON.parse(String(patch?.init?.body))).toEqual({
      version: 3,
      assetId: "asset-public",
    });
    expect(button(dialog, "当前封面").disabled).toBe(true);
  });
  test("切换英语翻译当前片库和详情，保留筛选值且不重复请求", async () => {
    const client = createBrowserMediaClient("shared", { readMode: "admin" });
    const host = await render(
      <MediaWorkspace
        client={client}
        imageLoader={browserImageLoader}
        canEdit={false}
        accountKey="admin"
      />,
    );
    await change(field(host, "搜索片库"), "typed draft");
    const count = requests.length;
    await act(async () => setLanguage("en"));
    expect(host.textContent).toContain("Search library");
    expect(field(host, "Search library").value).toBe("typed draft");
    expect(requests).toHaveLength(count);
    await click(button(host, "View details for 真实片名"));
    expect(host.textContent).toContain("Watching");
    expect(host.textContent).toContain("Overview");
    expect(host.textContent).not.toContain("entry.");
  });
});
