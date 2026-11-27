import { afterEach, beforeEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import type { MediaEntry, MediaList } from "@assemble/media";
import { createBrowserMediaClient } from "@assemble/media/browser-client";
import { act } from "react";
import { App } from "@/App";
import type { MediaBridge } from "@/lib/bridge";
import {
  button,
  cleanup,
  click,
  render,
  settle,
  text,
  typeInto,
} from "@/test-support/dom";
import { FakeBridge, snapshot } from "@/test-support/fakes";
import samples from "../../../packages/media/contract/snapshots.json";

beforeEach(async () => {
  await act(async () => setLanguage("zh-CN"));
});
afterEach(async () => {
  await cleanup();
  await act(async () => setLanguage("zh-CN"));
});
function fakeMedia() {
  const calls: string[] = [];
  const fetcher = (async (input: string | URL | Request) => {
    const path = String(input);
    calls.push(path);
    if (path === "/v1/media/capabilities")
      return Response.json({
        ...samples.capabilities,
        canMaintainPrivate: true,
      });
    if (path.includes("/entries?")) {
      const scope = path.includes("/private/") ? "private" : "shared";
      const entry = {
        ...samples.entries[scope === "private" ? 1 : 0],
        scope,
        title: scope === "private" ? "私人片名" : "共享片名",
      };
      return Response.json({ ...samples.listing, items: [entry] });
    }
    if (path.startsWith("/v1/media/private/entries/"))
      return Response.json({
        item: { ...samples.entries[1], title: "私人片名" },
      });
    return Response.json({ code: "NOT_FOUND" }, { status: 404 });
  }) as typeof fetch;
  const source: MediaBridge = {
    client: (scope) => createBrowserMediaClient(scope, { fetch: fetcher }),
    image: async () => "data:image/png;base64,YQ==",
    copyMagnet: async () => {
      throw new Error("unused test copy");
    },
  };
  return { calls, source };
}
test("桌面只维护本人片库，未登录不发片库请求，可打开全站网页", async () => {
  const auth = new FakeBridge(snapshot());
  const media = fakeMedia();
  const view = await render(<App bridge={auth} mediaBridge={media.source} />);
  await settle();
  await click(button(view.host, "影视"));
  await settle();
  expect(text(view.host)).toContain("登录后管理私人片库");
  expect(text(view.host)).not.toContain("共享片名");
  expect(view.host.querySelector('[role="tablist"]')).toBeNull();
  expect(media.calls).toHaveLength(0);
  await act(async () => setLanguage("en"));
  expect(text(view.host)).toContain("Sign in to manage your library");
});
test("退出账户立即卸载私人列表和仍在等待的旧请求", async () => {
  const auth = new FakeBridge(snapshot({ phase: "signedIn" }));
  const media = fakeMedia();
  let privateSignal: AbortSignal | undefined;
  let resolvePrivate: (value: MediaList) => void = () => {};
  const delayed: MediaBridge = {
    ...media.source,
    client: (scope) => {
      const client = media.source.client(scope);
      if (scope === "shared") return client;
      return {
        ...client,
        list: async (_query, signal) => {
          privateSignal = signal;
          return new Promise<MediaList>((resolve) => {
            resolvePrivate = resolve;
          });
        },
      };
    },
  };
  const view = await render(<App bridge={auth} mediaBridge={delayed} />);
  await settle();
  await click(button(view.host, "影视"));
  await settle();
  expect(privateSignal?.aborted).toBe(false);
  await act(async () => auth.push(snapshot()));
  await settle();
  expect(privateSignal?.aborted).toBe(true);
  await act(async () =>
    resolvePrivate({
      ...samples.listing,
      items: [
        {
          ...samples.entries[0],
          scope: "private",
          title: "迟到私人内容",
        } as MediaEntry,
      ],
    }),
  );
  await settle();
  expect(text(view.host)).not.toContain("迟到私人内容");
  expect(text(view.host)).toContain("登录后管理私人片库");
  expect(media.calls.some((path) => path.includes("/shared/"))).toBe(false);
});

test("登录后直接维护私人记录，浏览全站调用固定入口", async () => {
  const auth = new FakeBridge(snapshot({ phase: "signedIn" }));
  const media = fakeMedia();
  const view = await render(<App bridge={auth} mediaBridge={media.source} />);
  await settle();
  await click(button(view.host, "影视"));
  await settle();
  expect(text(view.host)).toContain("私人片名");
  expect(text(view.host)).not.toContain("共享片名");
  await click(button(view.host, "浏览全站片库"));
  expect(auth.calls).toContain("openPublicMedia");
  expect(media.calls.some((path) => path.includes("/shared/"))).toBe(false);
});

test("影视详情通过顶部面包屑返回原查询和页码，没有重复返回按钮，切换账户清除旧片名", async () => {
  const auth = new FakeBridge(snapshot({ phase: "signedIn" }));
  const media = fakeMedia();
  const source: MediaBridge = {
    ...media.source,
    client: (scope) => {
      const client = media.source.client(scope);
      return {
        ...client,
        list: async (query, signal) => ({
          ...(await client.list(query, signal)),
          total: 21,
          page: query?.page ?? 1,
          pageSize: query?.pageSize ?? 10,
        }),
      };
    },
  };
  const view = await render(<App bridge={auth} mediaBridge={source} />);
  await click(button(view.host, "影视"));
  await settle();
  const region = view.host.querySelector<HTMLElement>("#view-media");
  if (!region) throw new Error("MISSING_MEDIA_REGION");
  await typeInto(region, "私人");
  await click(button(region, "搜索"));
  await click(button(region, "下一页"));
  const listReads = media.calls.filter((path) =>
    path.includes("/entries?"),
  ).length;
  await click(button(region, "查看 私人片名 的详情"));
  await settle();
  expect(
    view.host.querySelector('header [data-slot="breadcrumb-page"]')
      ?.textContent,
  ).toBe("私人片名");
  expect(button(region, "返回片库")).toBeUndefined();
  expect(
    view.host.querySelectorAll('header [aria-current="page"]'),
  ).toHaveLength(1);
  await act(async () => setLanguage("en"));
  const parent = view.host.querySelector<HTMLAnchorElement>(
    'header a[href="#media"]',
  );
  if (!parent) throw new Error("MISSING_MEDIA_ANCESTOR");
  expect(parent.textContent).toBe("Movies & TV");
  expect(
    view.host.querySelector('header [data-slot="breadcrumb-page"]')
      ?.textContent,
  ).toBe("私人片名");
  await click(parent);
  await act(
    async () =>
      new Promise<void>((resolve) =>
        window.requestAnimationFrame(() => resolve()),
      ),
  );
  expect(
    region.querySelector<HTMLInputElement>('input[type="search"]')?.value,
  ).toBe("私人");
  expect(region.textContent).toContain("Page 2 of 3");
  expect(media.calls.filter((path) => path.includes("/entries?")).length).toBe(
    listReads,
  );
  expect(document.activeElement).toBe(
    view.host.querySelector('header [data-slot="breadcrumb-page"]'),
  );
  await click(button(region, "View details for 私人片名"));
  await act(async () => auth.push(snapshot()));
  await settle();
  expect(
    view.host.querySelector('header [data-slot="breadcrumb-page"]')
      ?.textContent,
  ).toBe("Movies & TV");
  expect(view.host.querySelector('header a[href="#media"]')).toBeNull();
  expect(region.textContent).not.toContain("私人片名");
});
