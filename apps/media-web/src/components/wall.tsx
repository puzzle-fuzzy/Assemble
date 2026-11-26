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
import { useShelf } from "../lib/use-shelf";
import { Pagination, RequestError } from "./common";
import { PosterCard } from "./poster";
import { ShelfSkeleton } from "./shelf-skeleton";

export function LibraryWall({
  client,
  query,
  pages,
  imageLoader,
  sessionSignal,
}: {
  client: MediaClient;
  query: MediaQuery;
  pages: ShelfPages;
  imageLoader: ImageLoader;
  sessionSignal: AbortSignal;
}) {
  const { t } = useSiteTranslation();
  const planned = useShelf(
    client,
    query,
    "planned",
    pages.planned,
    sessionSignal,
  );
  const watching = useShelf(
    client,
    query,
    "watching",
    pages.watching,
    sessionSignal,
  );
  const completed = useShelf(
    client,
    query,
    "completed",
    pages.completed,
    sessionSignal,
  );
  const shelves = [
    { status: "planned", ...planned },
    { status: "watching", ...watching },
    { status: "completed", ...completed },
  ] as const;
  useEffect(() => {
    const next = { ...pages };
    for (const { status, resource, pending } of [
      { status: "planned", ...planned },
      { status: "watching", ...watching },
      { status: "completed", ...completed },
    ] as const) {
      if (!pending && resource.phase === "ready")
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
        shelves.map(({ status, resource, pending, retry }) => {
          if (resource.phase === "ready" && resource.value.total === 0)
            return null;
          const title = t(status === "completed" ? "recentlyWatched" : status);
          return (
            <section
              key={status}
              className="home-shelf"
              aria-labelledby={`shelf-${status}`}
              aria-busy={pending}
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
                <ShelfSkeleton />
              ) : resource.phase === "failed" ? (
                <RequestError status={resource.status} retry={retry} />
              ) : (
                <>
                  <div
                    className={`poster-grid${resource.value.total > resource.value.pageSize ? " shelf-grid" : ""}`}
                    inert={pending}
                  >
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
                    pending={pending}
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
