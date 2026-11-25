import { describe, expect, test } from "bun:test";
import samples from "../contract/messages.json";
import { Controller, type ExtensionApi, merge, tabKey } from "./controller";
import { discover } from "./detector";
import {
  isNativeReply,
  sourceKind,
  sourceView,
  type TabSources,
} from "./protocol";

function fixture() {
  const data: Record<string, unknown> = {};
  const calls: { host: string; message: Record<string, unknown> }[] = [];
  const state = {
    url: "https://example.test/watch?code=private-page",
    allowed: true,
    fail: false,
  };
  const api = {
    storage: {
      session: {
        get: async (key: string | null) =>
          structuredClone(key ? { [key]: data[key] } : data),
        set: async (values: Record<string, unknown>) => {
          Object.assign(data, structuredClone(values));
        },
        remove: async (keys: string | string[]) => {
          for (const key of typeof keys === "string" ? [keys] : keys)
            delete data[key];
        },
      },
    },
    tabs: {
      query: async () => [{ id: 7, url: state.url }],
      get: async () => ({ id: 7, url: state.url }),
    },
    permissions: { contains: async () => state.allowed },
    runtime: {
      sendNativeMessage: async (
        host: string,
        message: Record<string, unknown>,
      ) => {
        calls.push({ host, message });
        if (state.fail) throw new Error("private native diagnostic");
        return { version: 1, ok: true, taskId: "task-1", errorCode: null };
      },
    },
    scripting: {
      executeScript: async (input: { files?: string[] }) =>
        input.files
          ? []
          : [
              {
                result: {
                  pageUrl: state.url,
                  title: "中文 <视频>",
                  sources: [
                    {
                      kind: "hls",
                      url: "https://cdn.example.test/video.m3u8?signature=private-media",
                      title: "中文 <视频>",
                    },
                  ],
                },
              },
            ],
    },
  } as unknown as ExtensionApi;
  return { api, data, calls, state, controller: new Controller(api) };
}
describe("浏览器来源与原生请求边界", () => {
  test("识别媒体、抖音与 MIME，拒绝 blob、私密用户信息和伪平台", () => {
    expect(sourceKind("https://cdn.example.test/video.M3U8?x=1")).toBe("hls");
    expect(
      sourceKind(
        "https://cdn.example.test/stream",
        "application/vnd.apple.mpegurl",
      ),
    ).toBe("hls");
    expect(sourceKind("https://cdn.example.test/movie.webm")).toBe("video");
    expect(sourceKind("https://v.douyin.com/abc/")).toBe("douyin");
    for (const url of [
      "blob:https://example.test/id",
      "file:///secret.mp4",
      "https://user:password@example.test/a.mp4",
      "https://douyin.com.evil.test/video/123456789012",
      "https://example.test/video.mpd",
    ])
      expect(sourceKind(url)).toBeNull();
    for (const reply of samples.replies)
      expect(isNativeReply(reply)).toBe(true);
    expect(isNativeReply({ ...samples.replies[0], token: "forbidden" })).toBe(
      false,
    );
  });
  test("DOM 与资源记录去重，保留中文标题，不执行网页内容", () => {
    const doc = document.implementation.createHTMLDocument("原始 <标题>");
    doc.title = "原始 <标题>";
    doc.head.insertAdjacentHTML(
      "beforeend",
      '<base href="https://example.test/watch">',
    );
    Object.defineProperty(doc, "location", {
      value: { href: "https://example.test/watch" },
    });
    doc.body.innerHTML =
      '<video src="/opaque?id=1"></video><video src="blob:https://example.test/id"></video><a href="https://cdn.example.test/a.m3u8?signature=private">HLS</a>';
    const result = discover(doc, [
      {
        name: "https://cdn.example.test/a.m3u8?signature=private",
      } as PerformanceEntry,
    ]);
    expect(result.sources).toHaveLength(2);
    expect(result.sources[0]?.url).toBe("https://example.test/opaque?id=1");
    expect(result.sources[0]?.title).toBe("原始 <标题>");
    expect(result.sources[1]?.kind).toBe("hls");
  });
  test("worker 重建保留来源，只将所选地址交给宿主，回复不含签名", async () => {
    const f = fixture();
    const first = await f.controller.handle({ type: "scan" });
    expect(first.ok).toBe(true);
    if (!first.ok || !first.list) throw new Error("scan");
    expect(JSON.stringify(first)).not.toContain("private-media");
    expect(JSON.stringify(first)).not.toContain("private-page");
    const id = first.list.items[0]?.id;
    if (!id) throw new Error("source");
    const restarted = new Controller(f.api);
    expect((await restarted.handle({ type: "list" })).ok).toBe(true);
    const submitted = await restarted.handle({ type: "download", id });
    expect(submitted.ok).toBe(true);
    expect(f.calls).toHaveLength(1);
    const request = f.calls[0]?.message;
    expect(request?.type).toBe("download");
    expect(request?.input).toEqual({
      kind: "hls",
      url: "https://cdn.example.test/video.m3u8?signature=private-media",
      title: "中文 <视频>",
      pageUrl: "https://example.test/watch",
    });
    await restarted.handle({ type: "download", id });
    expect(f.calls).toHaveLength(1);
    expect(JSON.stringify(submitted)).not.toContain("signature");
    f.state.url = "https://other.test/";
    expect(await restarted.handle({ type: "download", id })).toEqual({
      ok: false,
      code: "PAGE_UNAVAILABLE",
    });
  });
  test("仅捕获主动启用的当前页；可以停止，不接受其他标签的资源", async () => {
    const f = fixture();
    await f.controller.handle({ type: "scan" });
    const event = {
      tabId: 7,
      url: "https://cdn.example.test/opaque",
      mime: "video/mp4",
      bytes: 100,
    };
    await f.controller.record(event);
    expect((await f.controller.read(7))?.sources).toHaveLength(1);
    f.state.allowed = false;
    expect(
      await f.controller.handle({ type: "capture", enabled: true }),
    ).toEqual({ ok: false, code: "PERMISSION_DENIED" });
    f.state.allowed = true;
    await f.controller.handle({ type: "capture", enabled: true });
    await f.controller.record(event);
    expect((await f.controller.read(7))?.sources).toHaveLength(2);
    await f.controller.record({
      ...event,
      tabId: 99,
      url: "https://cdn.example.test/other.mp4",
    });
    expect(await f.controller.read(99)).toBeNull();
    await f.controller.handle({ type: "capture", enabled: false });
    await f.controller.record({
      ...event,
      url: "https://cdn.example.test/stopped.mp4",
    });
    expect((await f.controller.read(7))?.sources).toHaveLength(2);
  });
  test("过期、容量与错误诊断保持有界", async () => {
    const f = fixture();
    await f.controller.handle({ type: "scan" });
    const state = f.data[tabKey(7)] as TabSources;
    state.updatedAt = 0;
    expect(await f.controller.read(7)).toBeNull();
    await f.controller.handle({ type: "scan" });
    f.state.fail = true;
    const stored = f.data[tabKey(7)] as TabSources;
    const id = stored.sources[0]?.id;
    if (!id) throw new Error("source");
    expect(await f.controller.handle({ type: "download", id })).toEqual({
      ok: false,
      code: "BROWSER_NOT_CONNECTED",
    });
    merge(
      stored,
      Array.from({ length: 200 }, (_, i) => ({
        kind: "video" as const,
        url: `https://cdn.example.test/${i}.mp4?x=${"a".repeat(8000)}`,
        title: "标题",
      })),
    );
    expect(stored.sources.length).toBeLessThan(100);
    expect(JSON.stringify(sourceView(stored))).not.toContain("?x=");
  });
});
