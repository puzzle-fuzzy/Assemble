import type { MediaSession } from "@assemble/media";

interface HomePreloadEntry {
  promise: Promise<Response>;
  controller: AbortController;
}
interface HomePreload {
  origin: string;
  location: string;
  entries: Map<string, HomePreloadEntry>;
  dispose: () => void;
}
declare global {
  interface Window {
    assembleMediaHomePreload?: HomePreload;
  }
}

// 此函数会原样写成独立同源脚本；不能引用模块里的值或提前读取带会话的接口。
export function preloadAnonymousHome() {
  if (
    window.location.pathname !== "/movies/" ||
    !["http:", "https:"].includes(window.location.protocol) ||
    window.assembleMediaHomePreload
  )
    return;
  const search = new URLSearchParams(window.location.search);
  const positive = (value: string | null) => {
    const parsed = Number(value);
    return Number.isSafeInteger(parsed) && parsed > 0 && parsed <= 100_000
      ? parsed
      : 1;
  };
  const pages = {
    planned: positive(search.get("plannedPage") ?? search.get("page")),
    watching: positive(search.get("watchingPage")),
    completed: positive(search.get("completedPage")),
  };
  const mediaType = search.get("mediaType");
  const entries = new Map<string, HomePreloadEntry>();
  const controllers = new Set<AbortController>();
  const initial = window.location.pathname + window.location.search;
  const origin = window.location.origin;
  const changed = () => {
    if (
      window.location.origin !== origin ||
      window.location.pathname + window.location.search !== initial
    )
      state.dispose();
  };
  const state: HomePreload = {
    origin,
    location: initial,
    entries,
    dispose() {
      for (const controller of controllers) controller.abort();
      controllers.clear();
      entries.clear();
      window.removeEventListener("pagehide", state.dispose);
      window.removeEventListener("popstate", changed);
      window.removeEventListener("assemble:media-navigation", changed);
      if (window.assembleMediaHomePreload === state)
        delete window.assembleMediaHomePreload;
    },
  };
  window.assembleMediaHomePreload = state;
  window.addEventListener("pagehide", state.dispose, { once: true });
  window.addEventListener("popstate", changed);
  window.addEventListener("assemble:media-navigation", changed);
  for (const status of ["planned", "watching", "completed"] as const) {
    const url = new URL("/v1/media/public/entries", origin);
    if (mediaType === "movie" || mediaType === "tv")
      url.searchParams.set("mediaType", mediaType);
    url.searchParams.set("status", status);
    url.searchParams.set("page", String(pages[status]));
    url.searchParams.set("pageSize", "12");
    url.searchParams.set(
      "sort",
      status === "completed" ? "watched" : "updated",
    );
    url.searchParams.sort();
    const controller = new AbortController();
    controllers.add(controller);
    const timeout = window.setTimeout(
      () =>
        controller.abort(
          new DOMException("MEDIA_PRELOAD_TIMEOUT", "TimeoutError"),
        ),
      15_000,
    );
    const promise = new Promise<Response>((resolve, reject) => {
      const aborted = () => {
        window.clearTimeout(timeout);
        reject(controller.signal.reason);
      };
      controller.signal.addEventListener("abort", aborted, { once: true });
      void Promise.resolve()
        .then(() => {
          controller.signal.throwIfAborted();
          return window.fetch(url.href, {
            method: "GET",
            credentials: "omit",
            redirect: "error",
            cache: "no-store",
            headers: { Accept: "application/json" },
            signal: controller.signal,
          });
        })
        .then(async (response) => {
          // 预算覆盖完整正文；原响应留给已有 transport 校验与解码。
          await response.clone().arrayBuffer();
          return response;
        })
        .then(resolve, reject)
        .finally(() => {
          window.clearTimeout(timeout);
          controller.signal.removeEventListener("abort", aborted);
          controllers.delete(controller);
        });
    });
    entries.set(url.href, { promise, controller });
    // 提前失败不会成为未处理拒绝；消费端仍收到原来的失败。
    void promise.catch(() => {});
  }
}

export function discardHomePreload() {
  if (typeof window !== "undefined") window.assembleMediaHomePreload?.dispose();
}

function consume(entry: HomePreloadEntry, signal?: AbortSignal | null) {
  return new Promise<Response>((resolve, reject) => {
    const aborted = () => {
      entry.controller.abort(signal?.reason);
      reject(signal?.reason ?? new DOMException("Aborted", "AbortError"));
    };
    if (signal?.aborted) {
      aborted();
      return;
    }
    signal?.addEventListener("abort", aborted, { once: true });
    void entry.promise
      .then((response) => {
        if (signal?.aborted) aborted();
        else resolve(response);
      }, reject)
      .finally(() => signal?.removeEventListener("abort", aborted));
  });
}

// 只能传入已由服务端核验的 session；登录用户必须重新请求其自己的投影。
export function createHomePreloadFetch(
  session: MediaSession,
  fallback: typeof fetch = globalThis.fetch,
): typeof fetch {
  if (session.account !== null) {
    discardHomePreload();
    return fallback;
  }
  const bound =
    typeof window === "undefined" ? undefined : window.assembleMediaHomePreload;
  return ((input: RequestInfo | URL, init?: RequestInit) => {
    const state = bound;
    if (!state || window.assembleMediaHomePreload !== state)
      return fallback(input, init);
    if (
      window.location.origin !== state.origin ||
      window.location.pathname + window.location.search !== state.location
    ) {
      state.dispose();
      return fallback(input, init);
    }
    try {
      const request =
        typeof input === "object" && "url" in input && "method" in input
          ? (input as Request)
          : undefined;
      const method = init?.method ?? request?.method ?? "GET";
      const credentials = init?.credentials ?? request?.credentials;
      const headers = new Headers(init?.headers ?? request?.headers);
      const url = new URL(request?.url ?? String(input), window.location.href);
      if (
        method.toUpperCase() !== "GET" ||
        credentials === "include" ||
        headers.has("Authorization") ||
        headers.has("Cookie") ||
        url.origin !== state.origin ||
        url.username ||
        url.password ||
        url.hash
      )
        return fallback(input, init);
      url.searchParams.sort();
      const entry = state.entries.get(url.href);
      if (!entry) return fallback(input, init);
      state.entries.delete(url.href);
      const signal = init && "signal" in init ? init.signal : request?.signal;
      return consume(entry, signal);
    } catch {
      return fallback(input, init);
    }
  }) as typeof fetch;
}
