import type {
  ImageLoader,
  MediaClient,
  MediaQuery,
  MediaStatus,
} from "@assemble/media";
import { ToggleGroup, ToggleGroupItem } from "@assemble/ui/toggle-group";
import { useEffect } from "react";
import { useSiteTranslation } from "../lib/i18n";
import { navigate, type ShelfPages, wallPath } from "../lib/navigation";
import { useResource } from "../lib/use-resource";
import { LoadingContent, Pagination, RequestError } from "./common";
import { PosterCard } from "./poster";

function useShelf(
  client: MediaClient,
  query: MediaQuery,
  status: MediaStatus,
  page: number,
) {
  const request: MediaQuery = {
    mediaType: query.mediaType,
    status,
    page,
    pageSize: 12,
    sort: status === "completed" ? "watched" : "updated",
  };
  return useResource(`${client.scope}:${JSON.stringify(request)}`, (signal) =>
    client.list(request, signal),
  );
}

export function LibraryWall({
  client,
  query,
  pages,
  imageLoader,
}: {
  client: MediaClient;
  query: MediaQuery;
  pages: ShelfPages;
  imageLoader: ImageLoader;
}) {
  const { t } = useSiteTranslation();
  const planned = useShelf(client, query, "planned", pages.planned);
  const watching = useShelf(client, query, "watching", pages.watching);
  const completed = useShelf(client, query, "completed", pages.completed);
  const shelves = [
    { status: "planned", ...planned },
    { status: "watching", ...watching },
    { status: "completed", ...completed },
  ] as const;
  useEffect(() => {
    const next = { ...pages };
    for (const { status, resource } of [
      { status: "planned", ...planned },
      { status: "watching", ...watching },
      { status: "completed", ...completed },
    ] as const) {
      if (resource.phase === "ready")
        next[status] = Math.min(
          pages[status],
          Math.max(
            1,
            Math.ceil(resource.value.total / resource.value.pageSize),
          ),
        );
    }
    if (
      Object.keys(pages).some(
        (key) => next[key as MediaStatus] !== pages[key as MediaStatus],
      )
    )
      navigate(wallPath(client.scope, query, next), true);
  }, [client.scope, query, pages, planned, watching, completed]);
  const empty = shelves.every(
    ({ resource }) => resource.phase === "ready" && resource.value.total === 0,
  );
  const failed = shelves.every(({ resource }) => resource.phase === "failed");
  const returnTo = wallPath(client.scope, query, pages);
  return (
    <div className="home-shelves">
      <h1 className="sr-only">{t(client.scope)}</h1>
      <ToggleGroup
        type="single"
        value={query.mediaType ?? "all"}
        aria-label={t("mediaType")}
        onValueChange={(value) => {
          if (value === "all" || value === "movie" || value === "tv")
            navigate(
              wallPath(
                client.scope,
                value === "all" ? {} : { mediaType: value },
              ),
            );
        }}
      >
        <ToggleGroupItem value="all">{t("all")}</ToggleGroupItem>
        <ToggleGroupItem value="movie">{t("movie")}</ToggleGroupItem>
        <ToggleGroupItem value="tv">{t("tv")}</ToggleGroupItem>
      </ToggleGroup>
      {failed ? (
        <RequestError
          status={
            planned.resource.phase === "failed" ? planned.resource.status : null
          }
          retry={() => {
            planned.retry();
            watching.retry();
            completed.retry();
          }}
        />
      ) : empty ? (
        <p className="site-empty" role="status">
          {t("noEntries")}
        </p>
      ) : (
        shelves.map(({ status, resource, retry }) => {
          if (resource.phase === "ready" && resource.value.total === 0)
            return null;
          const title = t(status === "completed" ? "recentlyWatched" : status);
          return (
            <section
              key={status}
              className="home-shelf"
              aria-labelledby={`shelf-${status}`}
              aria-busy={resource.phase === "loading"}
            >
              <div className="shelf-heading">
                <h2 id={`shelf-${status}`}>{title}</h2>
                {resource.phase === "ready" && (
                  <span className="shelf-count">
                    {t("shelfCount", { count: resource.value.total })}
                  </span>
                )}
              </div>
              {resource.phase === "loading" ? (
                <LoadingContent wall />
              ) : resource.phase === "failed" ? (
                <RequestError status={resource.status} retry={retry} />
              ) : (
                <>
                  <div className="poster-grid">
                    {resource.value.items.map((entry, index) => (
                      <PosterCard
                        key={entry.id}
                        item={entry}
                        imageLoader={imageLoader}
                        priority={status === "planned" && index < 4}
                        returnTo={returnTo}
                      />
                    ))}
                  </div>
                  <Pagination
                    compact
                    label={t("shelfPagination", { title })}
                    page={resource.value.page}
                    total={resource.value.total}
                    pageSize={resource.value.pageSize}
                    onPage={(page) =>
                      navigate(
                        wallPath(client.scope, query, {
                          ...pages,
                          [status]: page,
                        }),
                      )
                    }
                  />
                </>
              )}
            </section>
          );
        })
      )}
    </div>
  );
}
