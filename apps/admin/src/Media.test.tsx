import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import {
  browserImageLoader,
  createBrowserMediaClient,
  createBrowserMediaCollectionsClient,
} from "@assemble/media/browser-client";
import type { MediaClient } from "@assemble/media/client";
import { MediaCollectionsManager } from "@assemble/media/components/collections";
import { MediaDetails } from "@assemble/media/components/media-details";
import { MediaWorkspace } from "@assemble/media/components/workspace";
import type { MediaEntry, MediaJob, MediaList } from "@assemble/media/model";
import { act, useState } from "react";
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

const entry: MediaEntry = {
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
  if (path.startsWith("/v1/media/shared/entries?")) return json(list());
  if (path === `/v1/media/shared/entries/${entry.id}`)
    return json({ item: entry });
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
beforeEach(() => {
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
  await act(async () => setLanguage("zh-CN"));
});

describe("影视业务界面与权限", () => {
  test("详情中的刷新返回最新列表，不保留已过期的详情版本", async () => {
    const client = createBrowserMediaClient("shared");
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
      path.startsWith("/v1/media/shared/entries?")
        ? json(list([{ ...entry, title: "更新后片名", version: 4 }]))
        : response(path);
    await click(button(host, "刷新"));
    expect(host.textContent).not.toContain("服务端简介");
    expect(host.textContent).toContain("更新后片名");
  });
  test("管理入口读取真实片库，携带版本和CSRF更新三种状态；冲突保留输入", async () => {
    let conflict = true;
    handle = (path, init) =>
      path === `/v1/admin/media/entries/${entry.id}` && init?.method === "PUT"
        ? conflict
          ? json({ code: "VERSION_CONFLICT" }, 409)
          : json({ item: { ...entry, version: 4, status: "completed" } })
        : response(path);
    const host = await render(<App />);
    await navigate(host, "media");
    await waitForText(host, "真实片名");
    expect(document.title).toContain("影视片库");
    await click(button(host, "查看 真实片名 的详情"));
    await click(button(host, "编辑"));
    const editor = document.body.querySelector('[role="dialog"]');
    if (!editor) throw new Error("missing editor");
    await change(field(editor, "观看状态"), "completed");
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
    await change(field(editor, "观看状态"), "completed");
    await click(button(document.body, "保存"));
    expect(document.body.querySelector('[role="dialog"]')).toBeNull();
    expect(host.textContent).toContain("已看");
  });
  test("失效会话清除管理内容，来源错误正文不进入页面", async () => {
    const host = await render(<App />);
    await navigate(host, "media");
    await waitForText(host, entry.title);
    handle = (path) =>
      path.startsWith("/v1/media/shared/entries?")
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
        path === `/v1/media/shared/entries/${entry.id}`
          ? new Promise((done) => {
              resolve = done;
              signal = init?.signal;
            })
          : response(path);
      await click(button(host, "复制磁力链接"));
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
      path === `/v1/media/shared/entries/${entry.id}`
        ? new Promise((done) => {
            resolve = done;
            signal = init?.signal;
          })
        : response(path);
    const client = createBrowserMediaClient("shared");
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
    expect(host.textContent).toContain("服务端简介");
  });
  test("合集从共享库选择并维护实际顺序，保存只提交成员标识", async () => {
    const second = { ...entry, id: "film-2", title: "第二部" };
    handle = (path, init) =>
      path.startsWith("/v1/media/shared/entries?")
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
        libraryClient={createBrowserMediaClient("shared", options)}
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
    await click(button(dialog, "上移 第二部"));
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
    expect(host.textContent).toContain(entry.overview);
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
      path === `/v1/media/shared/entries/${entry.id}/gallery`
        ? json({ entryId: entry.id, version: 3, items: [image] })
        : path === `/v1/admin/media/entries/${entry.id}/poster` &&
            init?.method === "PATCH"
          ? json({ item: { ...entry, version: 4, poster: image } })
          : response(path);
    const host = await render(<App />);
    await navigate(host, "media");
    await click(button(host, "查看 真实片名 的详情"));
    await click(button(host, "图集与封面"));
    const dialog = document.body.querySelector<HTMLElement>('[role="dialog"]');
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
    const client = createBrowserMediaClient("shared");
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
    expect(host.textContent).toContain("Shared library");
    expect(field(host, "Search library").value).toBe("typed draft");
    expect(requests).toHaveLength(count);
    await click(button(host, "View details for 真实片名"));
    expect(host.textContent).toContain("Watching");
    expect(host.textContent).toContain("Overview");
    expect(host.textContent).not.toContain("entry.");
  });
});
