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
        ...samples.entries[0],
        scope,
        title: scope === "private" ? "私人片名" : "共享片名",
      };
      return Response.json({ ...samples.listing, items: [entry] });
    }
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
async function selectPrivate(host: HTMLElement) {
  const tab = button(host, "我的片库");
  if (!tab) throw new Error("private tab missing");
  await act(async () => tab.focus());
  await settle();
}
test("未登录也能从桌面浏览共享影视，私人入口不发未经授权请求", async () => {
  const auth = new FakeBridge(snapshot());
  const media = fakeMedia();
  const view = await render(<App bridge={auth} mediaBridge={media.source} />);
  await settle();
  await click(button(view.host, "影视"));
  await settle();
  expect(text(view.host)).toContain("共享片名");
  expect(text(view.host)).not.toContain("登录 Assemble");
  await selectPrivate(view.host);
  expect(text(view.host)).toContain("登录后管理私人片库");
  expect(media.calls.some((path) => path.includes("/private/"))).toBe(false);
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
  await selectPrivate(view.host);
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
  expect(text(view.host)).toContain("共享片名");
});
