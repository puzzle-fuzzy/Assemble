import type {
  MediaClient,
  MediaCollectionContent,
  MediaCollectionsClient,
} from "./client";
import {
  isMediaActorCredits,
  isMediaCandidate,
  isMediaCapabilities,
  isMediaCollection,
  isMediaCollectionList,
  isMediaEntry,
  isMediaGallery,
  isMediaJob,
  isMediaList,
  isMediaSession,
  isMediaStatistics,
  requireMedia,
} from "./guard";
import type {
  ImageLoader,
  MediaCandidate,
  MediaIdentity,
  MediaQuery,
  MediaScope,
} from "./model";

export class MediaError extends Error {
  constructor(
    readonly code: string,
    readonly status?: number,
  ) {
    super(code);
    this.name = "MediaError";
  }
}
export interface BrowserMediaOptions {
  csrfToken?: () => string | null;
  fetch?: typeof fetch;
}
type Guard<T> = (value: unknown) => value is T;
function transport(options: BrowserMediaOptions = {}) {
  return async <T>(
    path: string,
    guard: Guard<T>,
    signal?: AbortSignal,
    method = "GET",
    body?: unknown,
  ): Promise<T> => {
    const headers: Record<string, string> = { Accept: "application/json" };
    if (method !== "GET") {
      const csrf = options.csrfToken?.();
      if (!csrf) throw new MediaError("CSRF_REQUIRED");
      headers["X-CSRF-Token"] = csrf;
      headers["Content-Type"] = "application/json";
    }
    const response = await (options.fetch ?? fetch)(path, {
      method,
      credentials: "same-origin",
      redirect: "error",
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      signal,
    });
    const raw = await response.text();
    if (raw.length > 4 * 1024 * 1024)
      throw new MediaError("INVALID_MEDIA_RESPONSE");
    let value: unknown;
    try {
      value = JSON.parse(raw);
    } catch {
      throw new MediaError("INVALID_MEDIA_RESPONSE");
    }
    if (!response.ok) {
      const code =
        value !== null &&
        typeof value === "object" &&
        "code" in value &&
        typeof value.code === "string" &&
        /^[A-Z][A-Z0-9_]{0,63}$/.test(value.code)
          ? value.code
          : `HTTP_${response.status}`;
      throw new MediaError(code, response.status);
    }
    return requireMedia(value, guard);
  };
}
const item =
  <T>(guard: Guard<T>): Guard<{ item: T }> =>
  (value): value is { item: T } =>
    value !== null &&
    typeof value === "object" &&
    "item" in value &&
    guard(value.item);
const candidates: Guard<{ items: MediaCandidate[] }> = (
  value,
): value is { items: MediaCandidate[] } =>
  value !== null &&
  typeof value === "object" &&
  "items" in value &&
  Array.isArray(value.items) &&
  value.items.every(isMediaCandidate);
const ok: Guard<{ ok: true }> = (value): value is { ok: true } =>
  value !== null &&
  typeof value === "object" &&
  "ok" in value &&
  value.ok === true;
function segment(value: string): string {
  if (!/^[A-Za-z0-9_.-]{1,200}$/.test(value) || value === "." || value === "..")
    throw new MediaError("INVALID_INPUT");
  return encodeURIComponent(value);
}
function query(value: object): string {
  const params = new URLSearchParams();
  for (const [key, part] of Object.entries(value))
    if (part !== undefined && part !== "") params.set(key, String(part));
  return params.size ? `?${params}` : "";
}
const sourceQuery = (identity: MediaIdentity) => query(identity);

export function createBrowserMediaClient(
  scope: MediaScope,
  options?: BrowserMediaOptions,
): MediaClient {
  const request = transport(options);
  const read = `/v1/media/${scope}`;
  const write = scope === "shared" ? "/v1/admin/media" : read;
  return {
    scope,
    capabilities: (signal) =>
      request("/v1/media/capabilities", isMediaCapabilities, signal),
    list: (filters: MediaQuery = {}, signal) =>
      request(`${read}/entries${query(filters)}`, isMediaList, signal),
    get: async (id, signal) =>
      (
        await request(
          `${read}/entries/${segment(id)}`,
          item(isMediaEntry),
          signal,
        )
      ).item,
    create: async (content, requestId, signal) =>
      (
        await request(`${write}/entries`, item(isMediaEntry), signal, "POST", {
          content,
          requestId,
        })
      ).item,
    update: async (id, version, content, signal) =>
      (
        await request(
          `${write}/entries/${segment(id)}`,
          item(isMediaEntry),
          signal,
          "PUT",
          { version, content },
        )
      ).item,
    remove: async (id, version, signal) => {
      await request(`${write}/entries/${segment(id)}`, ok, signal, "DELETE", {
        version,
      });
    },
    search: async (provider, mediaType, q, signal) =>
      (
        await request(
          `/v1/media/search${query({ provider, mediaType, q })}`,
          candidates,
          signal,
        )
      ).items,
    catalog: async (identity, signal) =>
      (
        await request(
          `/v1/media/catalog/details${sourceQuery(identity)}`,
          item(isMediaCandidate),
          signal,
        )
      ).item,
    actor: (identity, mediaType, signal) =>
      request(
        `/v1/media/catalog/actors/${identity.provider}/${segment(identity.externalId)}${query({ mediaType })}`,
        isMediaActorCredits,
        signal,
      ),
    import: (identity, requestId, signal) =>
      request(`${write}/imports`, isMediaJob, signal, "POST", {
        identity,
        requestId,
      }),
    job: (id, signal) =>
      request(`${write}/jobs/${segment(id)}`, isMediaJob, signal),
    gallery: (id, signal) =>
      request(`${read}/entries/${segment(id)}/gallery`, isMediaGallery, signal),
    syncGallery: (id, version, requestId, signal) =>
      request(
        `${write}/entries/${segment(id)}/gallery/sync`,
        isMediaJob,
        signal,
        "POST",
        { version, requestId },
      ),
    selectPoster: async (id, version, assetId, signal) =>
      (
        await request(
          `${write}/entries/${segment(id)}/poster`,
          item(isMediaEntry),
          signal,
          "PATCH",
          { version, assetId },
        )
      ).item,
  };
}
export function createBrowserMediaCollectionsClient(
  options?: BrowserMediaOptions,
): MediaCollectionsClient {
  const request = transport(options);
  const read = "/v1/media/collections";
  const write = "/v1/admin/media/collections";
  return {
    list: (filters = {}, signal) =>
      request(`${read}${query(filters)}`, isMediaCollectionList, signal),
    get: async (id, signal) =>
      (await request(`${read}/${segment(id)}`, item(isMediaCollection), signal))
        .item,
    create: async (content: MediaCollectionContent, requestId, signal) =>
      (
        await request(write, item(isMediaCollection), signal, "POST", {
          ...content,
          requestId,
        })
      ).item,
    update: async (id, version, content, signal) =>
      (
        await request(
          `${write}/${segment(id)}`,
          item(isMediaCollection),
          signal,
          "PUT",
          { ...content, version },
        )
      ).item,
    remove: async (id, version, signal) => {
      await request(`${write}/${segment(id)}`, ok, signal, "DELETE", {
        version,
      });
    },
  };
}
export const getMediaSession = (signal?: AbortSignal) =>
  transport()("/v1/media/session", isMediaSession, signal);
export const getMediaStatistics = (signal?: AbortSignal) =>
  transport()("/v1/admin/media/statistics", isMediaStatistics, signal);
/** 浏览器只使用固定同源资产路径；第三方原图地址不进入公共契约。 */
export const browserImageLoader: ImageLoader = async (
  image,
  variant,
  signal,
) => {
  signal?.throwIfAborted();
  if (!["original", "200", "360", "480"].includes(variant))
    throw new MediaError("INVALID_INPUT");
  return `/v1/media/assets/${segment(image.assetId)}/content?variant=${variant}`;
};
