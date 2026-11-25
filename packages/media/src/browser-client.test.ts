import { describe, expect, test } from "bun:test";
import samples from "../contract/snapshots.json";
import {
  browserImageLoader,
  createBrowserMediaClient,
  createBrowserMediaCollectionsClient,
  MediaError,
} from "./browser-client";

function fake(value: unknown, status = 200) {
  const calls: { path: string; init: RequestInit | undefined }[] = [];
  const fetcher = (async (path: string | URL | Request, init?: RequestInit) => {
    calls.push({ path: String(path), init });
    return new Response(JSON.stringify(value), {
      status,
      headers: { "Content-Type": "application/json" },
    });
  }) as typeof fetch;
  return { calls, fetcher };
}
describe("浏览器影视传输边界", () => {
  test("匿名共享查询固定同源路径，并对所有筛选编码", async () => {
    const source = fake(samples.listing);
    const client = createBrowserMediaClient("shared", {
      fetch: source.fetcher,
    });
    await client.list({
      q: "电影 & ?",
      status: "watching",
      page: 2,
      pageSize: 24,
    });
    const call = source.calls[0];
    const url = new URL(call.path, "https://assemble.example");
    expect(url.pathname).toBe("/v1/media/shared/entries");
    expect(url.searchParams.get("q")).toBe("电影 & ?");
    expect(url.searchParams.get("status")).toBe("watching");
    expect(call.init?.credentials).toBe("same-origin");
    expect(call.init?.redirect).toBe("error");
    expect(JSON.stringify(call.init)).not.toContain("Authorization");
  });
  test("维护在发送前要求CSRF；令牌只进入同源请求头", async () => {
    const source = fake({ item: samples.entries[0] });
    const entry = samples.entries[0];
    const content = {
      mediaType: "movie" as const,
      title: entry.title,
      originalTitle: "",
      year: null,
      releaseDate: null,
      overview: "",
      genres: [],
      episodeCount: null,
      status: "planned" as const,
      watchedAt: null,
      review: null,
      notes: null,
      magnetUri: null,
    };
    await expect(
      createBrowserMediaClient("shared", { fetch: source.fetcher }).create(
        content,
        "request-1",
      ),
    ).rejects.toMatchObject({ code: "CSRF_REQUIRED" });
    expect(source.calls).toHaveLength(0);
    await createBrowserMediaClient("shared", {
      fetch: source.fetcher,
      csrfToken: () => "csrf-test",
    }).create(content, "request-1");
    expect(source.calls[0].path).toBe("/v1/admin/media/entries");
    expect(source.calls[0].init?.headers).toMatchObject({
      "X-CSRF-Token": "csrf-test",
    });
    expect(JSON.parse(String(source.calls[0].init?.body))).toEqual({
      content,
      requestId: "request-1",
    });
  });
  test("错误只交出稳定码与状态，拒绝坏形状", async () => {
    const source = fake(
      { code: "UNAUTHORIZED", message: "private diagnostic" },
      401,
    );
    const error = await createBrowserMediaClient("private", {
      fetch: source.fetcher,
    })
      .get("id")
      .catch((value: unknown) => value);
    expect(error).toBeInstanceOf(MediaError);
    expect(error).toMatchObject({
      code: "UNAUTHORIZED",
      status: 401,
      message: "UNAUTHORIZED",
    });
    const invalid = fake({ items: [], total: 0 });
    await expect(
      createBrowserMediaClient("shared", { fetch: invalid.fetcher }).list(),
    ).rejects.toThrow("INVALID_MEDIA_RESPONSE");
  });
  test("合集详情使用独立公共入口；图像只生成编码后的固定资产路径", async () => {
    const source = fake({ item: samples.collection });
    await createBrowserMediaCollectionsClient({ fetch: source.fetcher }).get(
      "collection-1",
    );
    expect(source.calls[0].path).toBe("/v1/media/collections/collection-1");
    await expect(
      browserImageLoader(
        {
          assetId: "x?outside=https://evil.example",
          width: 10,
          height: 10,
          contentType: "image/jpeg",
        },
        "200",
      ),
    ).rejects.toMatchObject({ code: "INVALID_INPUT" });
    for (const id of [".", "..", "../private", "x\\private", "x\u0000"])
      await expect(
        createBrowserMediaClient("shared", { fetch: source.fetcher }).get(id),
      ).rejects.toMatchObject({ code: "INVALID_INPUT" });
    const abort = new AbortController();
    abort.abort();
    await expect(
      browserImageLoader(
        { assetId: "a", width: 1, height: 1, contentType: "image/png" },
        "original",
        abort.signal,
      ),
    ).rejects.toThrow();
  });
});
