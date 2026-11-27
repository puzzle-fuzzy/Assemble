import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { createPublicMediaClient } from "@assemble/media";
import samples from "../../../../packages/media/contract/snapshots.json";
import {
  createHomePreloadFetch,
  discardHomePreload,
  preloadAnonymousHome,
} from "./home-preload";
import { navigate, readRoute } from "./navigation";

const originalFetch = window.fetch;
const originalTimeout = window.setTimeout;
const originalClearTimeout = window.clearTimeout;
const calls: { url: URL; init?: RequestInit }[] = [];
let previousUrl = "about:blank";
let respond: (url: URL, init?: RequestInit) => Promise<Response>;

function setUrl(path: string) {
  (
    window as unknown as { happyDOM: { setURL: (url: string) => void } }
  ).happyDOM.setURL(new URL(path, "https://assemble.example").href);
}
function json(value: unknown = samples.listing) {
  return new Response(JSON.stringify(value), {
    headers: { "Content-Type": "application/json" },
  });
}
async function started() {
  await Promise.resolve();
  await Promise.resolve();
}
const plannedPath =
  "/v1/media/public/entries?status=planned&page=1&pageSize=12&sort=updated";
function fallback() {
  return mock(async () =>
    json({ fallback: true }),
  ) as unknown as typeof fetch & {
    mock: { calls: unknown[][] };
  };
}
function fakeTimeouts() {
  const handlers: (() => void)[] = [];
  window.setTimeout = ((handler: () => void, delay?: number) => {
    expect(delay).toBe(15_000);
    handlers.push(handler);
    return handlers.length;
  }) as typeof window.setTimeout;
  window.clearTimeout = mock(() => {});
  return handlers;
}

beforeEach(() => {
  previousUrl = window.location.href;
  setUrl("/movies/");
  calls.length = 0;
  respond = async () => json();
  window.fetch = mock((input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input), window.location.href);
    calls.push({ url, init });
    return respond(url, init);
  }) as unknown as typeof fetch;
});
afterEach(() => {
  discardHomePreload();
  window.fetch = originalFetch;
  window.setTimeout = originalTimeout;
  window.clearTimeout = originalClearTimeout;
  (
    window as unknown as { happyDOM: { setURL: (url: string) => void } }
  ).happyDOM.setURL(previousUrl);
});

test("serialized external script starts exactly three anonymous shared shelf requests", async () => {
  setUrl("/movies/?mediaType=tv&plannedPage=4&watchingPage=2&completedPage=3");
  new Function(`(${preloadAnonymousHome.toString()})();`)();
  await started();
  expect(calls).toHaveLength(3);
  expect(calls.map(({ url }) => Object.fromEntries(url.searchParams))).toEqual([
    {
      mediaType: "tv",
      status: "planned",
      page: "4",
      pageSize: "12",
      sort: "updated",
    },
    {
      mediaType: "tv",
      status: "watching",
      page: "2",
      pageSize: "12",
      sort: "updated",
    },
    {
      mediaType: "tv",
      status: "completed",
      page: "3",
      pageSize: "12",
      sort: "watched",
    },
  ]);
  for (const { url, init } of calls) {
    expect(url.origin).toBe("https://assemble.example");
    expect(url.pathname).toBe("/v1/media/public/entries");
    expect(init).toMatchObject({
      method: "GET",
      credentials: "omit",
      redirect: "error",
      cache: "no-store",
      headers: { Accept: "application/json" },
    });
    expect(init?.signal).toBeInstanceOf(AbortSignal);
  }
  preloadAnonymousHome();
  await started();
  expect(calls).toHaveLength(3);
});

test("preload never starts for private, detail, source, collection, or lookalike paths", async () => {
  for (const path of [
    "/movies",
    "/movies//",
    "/movies/private",
    "/movies/users/alice",
    "/movies/entries/shared/one",
    "/movies/entries/private/one",
    "/movies/catalog/tmdb/movie/42",
    "/movies/actors/tmdb/42",
    "/movies/collections",
    "/movies/collections/one",
    "/other/",
  ]) {
    setUrl(path);
    preloadAnonymousHome();
  }
  await started();
  expect(calls).toHaveLength(0);
  expect(window.assembleMediaHomePreload).toBeUndefined();
});

test("preload pages and filters exactly match readRoute including legacy and invalid queries", async () => {
  for (const search of [
    "?page=4",
    "?page=4&plannedPage=2",
    "?page=4&plannedPage=",
    "?plannedPage=100000&watchingPage=1e2&completedPage=0x10&mediaType=movie",
    "?plannedPage=100001&watchingPage=-1&completedPage=1.2&mediaType=invalid",
    "?plannedPage=2&plannedPage=3&watchingPage=NaN&completedPage=0",
  ]) {
    setUrl(`/movies/${search}`);
    const route = readRoute(window.location.href);
    if (route.kind !== "wall") throw new Error("EXPECTED_WALL");
    calls.length = 0;
    preloadAnonymousHome();
    await started();
    for (const { url } of calls) {
      const status = url.searchParams.get("status") as keyof typeof route.pages;
      expect(Number(url.searchParams.get("page"))).toBe(route.pages[status]);
      expect(url.searchParams.get("mediaType")).toBe(
        route.query.mediaType ?? null,
      );
    }
    discardHomePreload();
  }
});

test("anonymous transport consumes each matching response once and retains DTO validation", async () => {
  preloadAnonymousHome();
  const otherFetch = fallback();
  const fetcher = createHomePreloadFetch({ account: null }, otherFetch);
  const client = createPublicMediaClient({ fetch: fetcher });
  const query = {
    status: "planned" as const,
    page: 1,
    pageSize: 12,
    sort: "updated" as const,
  };
  expect<unknown>(await client.list(query)).toEqual(samples.listing);
  expect(calls).toHaveLength(3);
  expect(otherFetch.mock.calls).toHaveLength(0);
  await expect(client.list(query)).rejects.toThrow("INVALID_MEDIA_RESPONSE");
  expect(otherFetch.mock.calls).toHaveLength(1);
  expect(window.assembleMediaHomePreload?.entries.size).toBe(2);
});

test("invalid preloaded DTOs still fail the normal transport guard without another request", async () => {
  respond = async () => json({ items: [{ magnetUri: "invalid" }] });
  preloadAnonymousHome();
  const otherFetch = fallback();
  const client = createPublicMediaClient({
    fetch: createHomePreloadFetch({ account: null }, otherFetch),
  });
  await expect(
    client.list({ status: "planned", page: 1, pageSize: 12, sort: "updated" }),
  ).rejects.toThrow("INVALID_MEDIA_RESPONSE");
  expect(otherFetch.mock.calls).toHaveLength(0);
});

test("logged-in sessions discard and cancel anonymous preloads", async () => {
  respond = () => new Promise(() => {});
  preloadAnonymousHome();
  await started();
  const otherFetch = fallback();
  const fetcher = createHomePreloadFetch(
    {
      account: { id: "account-1", displayName: "测试账户", avatarUrl: null },
    },
    otherFetch,
  );
  expect(fetcher).toBe(otherFetch);
  expect(window.assembleMediaHomePreload).toBeUndefined();
  expect(calls.every(({ init }) => init?.signal?.aborted)).toBe(true);
});

test("unmatched, credentialed, private, changed-page and cross-origin requests never consume a preload", async () => {
  preloadAnonymousHome();
  const otherFetch = fallback();
  const fetcher = createHomePreloadFetch({ account: null }, otherFetch);
  for (const [path, init] of [
    [plannedPath, { method: "POST" }],
    [plannedPath, { credentials: "include" }],
    [plannedPath, { headers: { Authorization: "Bearer fixture" } }],
    [plannedPath, { headers: { Cookie: "fixture=1" } }],
    [plannedPath.replace("/public/", "/private/"), {}],
    [plannedPath.replace("page=1", "page=2"), {}],
    [plannedPath.replace("pageSize=12", "pageSize=24"), {}],
    [`https://other.example${plannedPath}`, {}],
    [`${plannedPath}#fragment`, {}],
    [`https://fixture:fixture@assemble.example${plannedPath}`, {}],
  ] as [string, RequestInit][])
    await fetcher(path, init);
  expect(otherFetch.mock.calls).toHaveLength(10);
  expect(window.assembleMediaHomePreload?.entries.size).toBe(3);
  await fetcher(
    new Request(new URL(plannedPath, window.location.href), {
      credentials: "same-origin",
      redirect: "error",
    }),
  );
  expect(otherFetch.mock.calls).toHaveLength(10);
  expect(window.assembleMediaHomePreload?.entries.size).toBe(2);
});

test("consumer abort cancels the matching request even when fetch ignores abort", async () => {
  respond = () => new Promise(() => {});
  preloadAnonymousHome();
  await started();
  const controller = new AbortController();
  const fetcher = createHomePreloadFetch({ account: null }, fallback());
  const request = fetcher(plannedPath, { signal: controller.signal });
  controller.abort();
  await expect(request).rejects.toMatchObject({ name: "AbortError" });
  expect(calls[0]?.init?.signal?.aborted).toBe(true);
  expect(calls[1]?.init?.signal?.aborted).toBe(false);
  expect(window.assembleMediaHomePreload?.entries.size).toBe(2);
});

test("an already-aborted consumer rejects and does not consume the same entry twice", async () => {
  respond = () => new Promise(() => {});
  preloadAnonymousHome();
  await started();
  const controller = new AbortController();
  controller.abort();
  const otherFetch = fallback();
  const fetcher = createHomePreloadFetch({ account: null }, otherFetch);
  await expect(
    fetcher(plannedPath, { signal: controller.signal }),
  ).rejects.toMatchObject({ name: "AbortError" });
  await fetcher(plannedPath);
  expect(otherFetch.mock.calls).toHaveLength(1);
  expect(calls[0]?.init?.signal?.aborted).toBe(true);
});

test("pagehide cancels both unconsumed and consumed in-flight requests and clears state", async () => {
  respond = () => new Promise(() => {});
  preloadAnonymousHome();
  await started();
  const request = createHomePreloadFetch(
    { account: null },
    fallback(),
  )(plannedPath);
  window.dispatchEvent(new Event("pagehide"));
  await expect(request).rejects.toMatchObject({ name: "AbortError" });
  expect(calls.every(({ init }) => init?.signal?.aborted)).toBe(true);
  expect(window.assembleMediaHomePreload).toBeUndefined();
});

test("navigation cancels old route preloads and an old fetch cannot consume newly created state", async () => {
  respond = () => new Promise(() => {});
  preloadAnonymousHome();
  await started();
  const otherFetch = fallback();
  const fetcher = createHomePreloadFetch({ account: null }, otherFetch);
  navigate("/movies/?plannedPage=2");
  expect(window.assembleMediaHomePreload).toBeUndefined();
  expect(calls.every(({ init }) => init?.signal?.aborted)).toBe(true);
  navigate("/movies/");
  preloadAnonymousHome();
  await started();
  await fetcher(plannedPath);
  expect(otherFetch.mock.calls).toHaveLength(1);
  expect(window.assembleMediaHomePreload?.entries.size).toBe(3);
  window.history.replaceState(null, "", "/movies/private");
  window.dispatchEvent(new Event("popstate"));
  expect(window.assembleMediaHomePreload).toBeUndefined();
});

test("15-second timeout rejects ignored requests and failures remain safely consumable", async () => {
  const timers = fakeTimeouts();
  respond = () => new Promise(() => {});
  preloadAnonymousHome();
  await started();
  for (const expire of timers) expire();
  await started();
  expect(calls.every(({ init }) => init?.signal?.aborted)).toBe(true);
  await expect(
    createHomePreloadFetch({ account: null }, fallback())(plannedPath),
  ).rejects.toMatchObject({ name: "TimeoutError" });
});

test("timeout covers the response body rather than only response headers", async () => {
  const timers = fakeTimeouts();
  const streams: ReadableStreamDefaultController<Uint8Array>[] = [];
  respond = async () =>
    new Response(
      new ReadableStream<Uint8Array>({
        start(controller) {
          streams.push(controller);
        },
      }),
    );
  preloadAnonymousHome();
  await started();
  const request = createHomePreloadFetch(
    { account: null },
    fallback(),
  )(plannedPath);
  for (const expire of timers) expire();
  await expect(request).rejects.toMatchObject({ name: "TimeoutError" });
  for (const stream of streams) stream.close();
});

test("early network rejection has no unhandled failure and is preserved for the transport", async () => {
  respond = async () => {
    throw new Error("NETWORK_UNAVAILABLE");
  };
  preloadAnonymousHome();
  await started();
  const otherFetch = fallback();
  await expect(
    createHomePreloadFetch({ account: null }, otherFetch)(plannedPath),
  ).rejects.toThrow("NETWORK_UNAVAILABLE");
  expect(otherFetch.mock.calls).toHaveLength(0);
});
