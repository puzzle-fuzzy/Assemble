import type {
  MediaIdentity,
  MediaPersonIdentity,
  MediaQuery,
  MediaScope,
  MediaStatus,
  MediaType,
} from "@assemble/media";
import { useEffect, useState } from "react";

export const loginPath = "/web/login?next=/movies/";
const navigationEvent = "assemble:media-navigation";

export type ShelfPages = Record<MediaStatus, number>;

export type SiteRoute =
  | { kind: "wall"; scope: MediaScope; query: MediaQuery; pages: ShelfPages }
  | { kind: "entry"; scope: MediaScope; id: string }
  | { kind: "catalog"; identity: MediaIdentity }
  | { kind: "actor"; identity: MediaPersonIdentity; mediaType: MediaType }
  | { kind: "collections"; page: number }
  | { kind: "collection"; id: string }
  | { kind: "missing" };

function positive(value: string | null) {
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed > 0 && parsed <= 100_000
    ? parsed
    : 1;
}

export function readQuery(search: URLSearchParams): MediaQuery {
  const mediaType = search.get("mediaType");
  return {
    page: positive(search.get("page")),
    pageSize: 12,
    ...(mediaType === "movie" || mediaType === "tv" ? { mediaType } : {}),
  };
}

function validId(value: string | undefined): value is string {
  return Boolean(
    value &&
      value.length <= 256 &&
      !/[\s/\\]/u.test(value) &&
      ![...value].some((character) => character.charCodeAt(0) < 32),
  );
}

export function readRoute(location: string): SiteRoute {
  const url = new URL(location, "http://localhost");
  let parts: string[];
  try {
    parts = url.pathname
      .replace(/\/$/, "")
      .split("/")
      .slice(2)
      .map(decodeURIComponent);
  } catch {
    return { kind: "missing" };
  }
  if (!url.pathname.startsWith("/movies/")) return { kind: "missing" };
  if (parts.length === 0 || (parts.length === 1 && parts[0] === "private")) {
    return {
      kind: "wall",
      scope: parts[0] === "private" ? "private" : "shared",
      query: readQuery(url.searchParams),
      pages: {
        planned: positive(
          url.searchParams.get("plannedPage") ?? url.searchParams.get("page"),
        ),
        watching: positive(url.searchParams.get("watchingPage")),
        completed: positive(url.searchParams.get("completedPage")),
      },
    };
  }
  if (
    parts.length === 3 &&
    parts[0] === "entries" &&
    (parts[1] === "shared" || parts[1] === "private") &&
    validId(parts[2])
  ) {
    return { kind: "entry", scope: parts[1], id: parts[2] };
  }
  if (parts[0] === "collections") {
    if (parts.length === 1)
      return {
        kind: "collections",
        page: positive(url.searchParams.get("page")),
      };
    if (parts.length === 2 && validId(parts[1]))
      return { kind: "collection", id: parts[1] };
  }
  if (parts[1] === "tmdb" || parts[1] === "douban") {
    if (
      parts[0] === "catalog" &&
      parts.length === 4 &&
      (parts[2] === "movie" || parts[2] === "tv") &&
      validId(parts[3])
    ) {
      return {
        kind: "catalog",
        identity: {
          provider: parts[1],
          mediaType: parts[2],
          externalId: parts[3],
        },
      };
    }
    if (parts[0] === "actors" && parts.length === 3 && validId(parts[2])) {
      return {
        kind: "actor",
        identity: { provider: parts[1], externalId: parts[2] },
        mediaType: url.searchParams.get("mediaType") === "tv" ? "tv" : "movie",
      };
    }
  }
  return { kind: "missing" };
}

export function wallPath(
  scope: MediaScope,
  query: MediaQuery = {},
  pages?: ShelfPages,
) {
  const parameters = new URLSearchParams();
  if (query.mediaType) parameters.set("mediaType", query.mediaType);
  if (pages) {
    for (const [status, page] of Object.entries(pages)) {
      if (page > 1) parameters.set(`${status}Page`, String(page));
    }
  } else if (query.page && query.page > 1) {
    parameters.set("plannedPage", String(query.page));
  }
  const search = parameters.toString();
  return `/movies/${scope === "private" ? "private" : ""}${search ? `?${search}` : ""}`;
}

export function readerBackPath(scope: MediaScope) {
  const from = new URLSearchParams(window.location.search).get("from");
  if (from?.startsWith("/movies/")) {
    const route = readRoute(from);
    if (route.kind === "wall" && route.scope === scope)
      return wallPath(scope, route.query, route.pages);
  }
  return wallPath(scope);
}
export const entryPath = (scope: MediaScope, id: string) =>
  `/movies/entries/${scope}/${encodeURIComponent(id)}`;
export const catalogPath = (identity: MediaIdentity) =>
  `/movies/catalog/${identity.provider}/${identity.mediaType}/${encodeURIComponent(identity.externalId)}`;
export const actorPath = (
  identity: MediaPersonIdentity,
  mediaType: MediaType,
) =>
  `/movies/actors/${identity.provider}/${encodeURIComponent(identity.externalId)}?mediaType=${mediaType}`;

export function navigate(path: string, replace = false) {
  if (!path.startsWith("/movies/")) throw new Error("INVALID_MEDIA_NAVIGATION");
  if (replace) window.history.replaceState(null, "", path);
  else window.history.pushState(null, "", path);
  window.dispatchEvent(new Event(navigationEvent));
}

const current = () => window.location.pathname + window.location.search;

export function useSiteLocation() {
  const [location, setLocation] = useState(current);
  useEffect(() => {
    const changed = () => setLocation(current());
    window.addEventListener("popstate", changed);
    window.addEventListener(navigationEvent, changed);
    return () => {
      window.removeEventListener("popstate", changed);
      window.removeEventListener(navigationEvent, changed);
    };
  }, []);
  return location;
}
