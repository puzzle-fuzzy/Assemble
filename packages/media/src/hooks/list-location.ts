import {
  setUrlParameter,
  type UrlStateCodec,
  urlInteger,
} from "@assemble/ui/hooks/use-url-state";
import type { MediaQuery } from "../model";

function choice<T extends string>(value: string | null, options: readonly T[]) {
  return options.find((option) => option === value);
}

export function mediaQueryCodec(pageSize: number): UrlStateCodec<MediaQuery> {
  return {
    read: (params) => ({
      page: urlInteger(params.get("page"), 1, 4_294_967_295),
      pageSize: urlInteger(params.get("pageSize"), pageSize, 100),
      q:
        (params.get("q") ?? "")
          .replace(/[\p{Cc}]/gu, "")
          .slice(0, 300)
          .trim() || undefined,
      sort:
        choice(params.get("sort"), ["updated", "title", "year", "watched"]) ??
        "updated",
      mediaType: choice(params.get("mediaType"), ["movie", "tv"]),
      status: choice(params.get("status"), [
        "planned",
        "watching",
        "completed",
      ]),
      scope: choice(params.get("scope"), ["shared", "private"]),
    }),
    write: (params, query) => {
      setUrlParameter(params, "page", query.page, 1);
      setUrlParameter(params, "pageSize", query.pageSize, pageSize);
      setUrlParameter(params, "q", query.q);
      setUrlParameter(params, "sort", query.sort, "updated");
      setUrlParameter(params, "mediaType", query.mediaType);
      setUrlParameter(params, "status", query.status);
      setUrlParameter(params, "scope", query.scope);
    },
  };
}

export function collectionQueryCodec(pageSize: number) {
  return {
    read: (params: URLSearchParams) => ({
      page: urlInteger(params.get("collectionPage"), 1, 4_294_967_295),
      pageSize: urlInteger(params.get("collectionPageSize"), pageSize, 100),
    }),
    write: (
      params: URLSearchParams,
      query: { page: number; pageSize: number },
    ) => {
      setUrlParameter(params, "collectionPage", query.page, 1);
      setUrlParameter(params, "collectionPageSize", query.pageSize, pageSize);
    },
  };
}
