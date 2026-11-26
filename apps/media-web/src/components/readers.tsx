import type {
  ImageLoader,
  MediaClient,
  MediaCollectionsClient,
} from "@assemble/media";
import { Badge } from "@assemble/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@assemble/ui/card";
import { ToggleGroup, ToggleGroupItem } from "@assemble/ui/toggle-group";
import { useEffect, useRef, useState } from "react";
import { useSiteTranslation } from "../lib/i18n";
import {
  actorPath,
  loginPath,
  navigate,
  readerBackPath,
  type SiteRoute,
} from "../lib/navigation";
import { useResource } from "../lib/use-resource";
import {
  BackLink,
  EmptyContentView,
  LoadingContent,
  Pagination,
  RequestError,
  SiteLink,
} from "./common";
import { DetailViewer, PosterViewer } from "./detail-viewer";
import { MediaImageView, PosterCard } from "./poster";
import "./detail-viewer.css";

type ReaderProps = { client: MediaClient; imageLoader: ImageLoader };

export function EntryReader({
  client,
  id,
  imageLoader,
  signedIn,
  sessionSignal,
}: ReaderProps & {
  id: string;
  signedIn: boolean;
  sessionSignal: AbortSignal;
}) {
  const { t } = useSiteTranslation();
  const { resource, retry } = useResource(`${client.scope}:${id}`, (signal) =>
    client.get(id, signal),
  );
  const copying = useRef<AbortController | null>(null);
  useEffect(() => () => copying.current?.abort(), []);
  return (
    <div className="flex flex-col gap-5">
      <BackLink href={readerBackPath(client.scope)} />
      {resource.phase === "loading" ? (
        <LoadingContent />
      ) : resource.phase === "failed" ? (
        <RequestError status={resource.status} retry={retry} />
      ) : (
        <>
          <DetailViewer
            key={`${client.scope}:${resource.value.id}`}
            client={client}
            sessionSignal={sessionSignal}
            entry={resource.value}
            imageLoader={imageLoader}
            onActor={(identity) =>
              navigate(actorPath(identity, resource.value.mediaType))
            }
            onCopyMagnet={
              signedIn
                ? async (entry) => {
                    copying.current?.abort();
                    const operation = new AbortController();
                    copying.current = operation;
                    const stop = () => operation.abort();
                    sessionSignal.addEventListener("abort", stop, {
                      once: true,
                    });
                    try {
                      sessionSignal.throwIfAborted();
                      const current = await client.get(
                        entry.id,
                        operation.signal,
                      );
                      operation.signal.throwIfAborted();
                      if (!current.magnetUri)
                        throw new Error("MAGNET_UNAVAILABLE");
                      await navigator.clipboard.writeText(current.magnetUri);
                    } finally {
                      sessionSignal.removeEventListener("abort", stop);
                    }
                  }
                : undefined
            }
            onSignIn={
              !signedIn ? () => window.location.assign(loginPath) : undefined
            }
          />
          <Title title={`${resource.value.title} · ${t("brand")}`} />
        </>
      )}
    </div>
  );
}

export function CatalogReader({
  client,
  route,
  imageLoader,
}: ReaderProps & { route: Extract<SiteRoute, { kind: "catalog" }> }) {
  const { t } = useSiteTranslation();
  const { resource, retry } = useResource(JSON.stringify(route), (signal) =>
    client.catalog(route.identity, signal),
  );
  return (
    <div className="flex flex-col gap-5">
      <BackLink />
      {resource.phase === "loading" ? (
        <LoadingContent />
      ) : resource.phase === "failed" ? (
        <RequestError status={resource.status} retry={retry} />
      ) : (
        <article className="flex flex-col gap-6">
          <div className="catalog-intro">
            <PosterViewer
              image={resource.value.poster}
              imageLoader={imageLoader}
              title={resource.value.title}
            />
            <header className="flex min-w-0 flex-col items-start gap-3">
              <Badge variant="outline">{t("catalog")}</Badge>
              <h1 className="site-heading">{resource.value.title}</h1>
              {resource.value.originalTitle &&
                resource.value.originalTitle !== resource.value.title && (
                  <p className="text-muted-foreground">
                    {resource.value.originalTitle}
                  </p>
                )}
              <p>
                {resource.value.year ?? t("unknownYear")} ·{" "}
                {t(resource.value.identity.mediaType)}
              </p>
            </header>
          </div>
          <section className="flex flex-col gap-2">
            <h2 className="text-lg font-semibold">{t("overview")}</h2>
            <p className="whitespace-pre-wrap leading-relaxed">
              {resource.value.overview || t("noOverview")}
            </p>
          </section>
          <Title title={`${resource.value.title} · ${t("brand")}`} />
        </article>
      )}
    </div>
  );
}

export function ActorReader({
  client,
  route,
  imageLoader,
}: ReaderProps & { route: Extract<SiteRoute, { kind: "actor" }> }) {
  const { t } = useSiteTranslation();
  const { resource, retry } = useResource(JSON.stringify(route), (signal) =>
    client.actor(route.identity, route.mediaType, signal),
  );
  const [page, setPage] = useState(1);
  const pageSize = 24;
  return (
    <section className="flex flex-col gap-6">
      <BackLink />
      <div className="flex gap-3">
        <label htmlFor="actor-type" className="sr-only">
          {t("mediaType")}
        </label>
        <ActorType route={route} />
      </div>
      {resource.phase === "loading" ? (
        <LoadingContent wall />
      ) : resource.phase === "failed" ? (
        <RequestError status={resource.status} retry={retry} />
      ) : (
        <>
          <header className="actor-heading">
            <MediaImageView
              image={resource.value.actor.photo}
              imageLoader={imageLoader}
              alt=""
              variant="200"
              className="actor-photo"
            />
            <h1 className="site-heading">{resource.value.actor.name}</h1>
          </header>
          {resource.value.items.length === 0 ? (
            <EmptyContentView title={t("noCredits")} />
          ) : (
            <div className="poster-grid">
              {resource.value.items
                .slice((page - 1) * pageSize, page * pageSize)
                .map((candidate) => (
                  <PosterCard
                    key={`${candidate.identity.provider}:${candidate.identity.mediaType}:${candidate.identity.externalId}`}
                    item={candidate}
                    imageLoader={imageLoader}
                  />
                ))}
            </div>
          )}
          <Pagination
            page={page}
            total={resource.value.items.length}
            pageSize={pageSize}
            onPage={setPage}
          />
          <Title
            title={`${resource.value.actor.name} · ${t("actors")} · ${t("brand")}`}
          />
        </>
      )}
    </section>
  );
}

function ActorType({
  route,
}: {
  route: Extract<SiteRoute, { kind: "actor" }>;
}) {
  const { t } = useSiteTranslation();
  return (
    <ToggleGroup
      id="actor-type"
      type="single"
      aria-label={t("mediaType")}
      value={route.mediaType}
      onValueChange={(type) => {
        if (type === "movie" || type === "tv")
          navigate(actorPath(route.identity, type));
      }}
    >
      <ToggleGroupItem value="movie">{t("movie")}</ToggleGroupItem>
      <ToggleGroupItem value="tv">{t("tv")}</ToggleGroupItem>
    </ToggleGroup>
  );
}

export function CollectionsReader({
  client,
  page,
  imageLoader,
}: {
  client: MediaCollectionsClient;
  page: number;
  imageLoader: ImageLoader;
}) {
  const { t } = useSiteTranslation();
  const { resource, retry } = useResource(`collections:${page}`, (signal) =>
    client.list({ page, pageSize: 12 }, signal),
  );
  useEffect(() => {
    if (resource.phase !== "ready") return;
    const pages = Math.max(
      1,
      Math.ceil(resource.value.total / resource.value.pageSize),
    );
    if (page > pages) navigate(`/movies/collections?page=${pages}`, true);
  }, [page, resource]);
  return (
    <section className="flex flex-col gap-6">
      <h1 className="site-heading">{t("collections")}</h1>
      {resource.phase === "loading" ? (
        <LoadingContent wall />
      ) : resource.phase === "failed" ? (
        <RequestError status={resource.status} retry={retry} />
      ) : (
        <>
          {resource.value.items.length === 0 ? (
            <EmptyContentView title={t("collectionsEmpty")} />
          ) : (
            <div className="collection-grid">
              {resource.value.items.map((collection) => (
                <Card key={collection.id}>
                  <CardHeader>
                    <CardTitle>
                      <SiteLink
                        className="hover:underline"
                        href={`/movies/collections/${encodeURIComponent(collection.id)}`}
                      >
                        {collection.title}
                      </SiteLink>
                    </CardTitle>
                    <CardDescription>
                      {t("collectionCount", { count: collection.items.length })}
                    </CardDescription>
                  </CardHeader>
                  <CardContent className="flex flex-col gap-3">
                    <MediaImageView
                      image={
                        collection.poster ?? collection.items[0]?.poster ?? null
                      }
                      imageLoader={imageLoader}
                      alt=""
                      className="collection-art"
                    />
                    <p className="line-clamp-3 text-sm text-muted-foreground">
                      {collection.description}
                    </p>
                  </CardContent>
                </Card>
              ))}
            </div>
          )}
          <Pagination
            page={resource.value.page}
            total={resource.value.total}
            pageSize={resource.value.pageSize}
            onPage={(page) => navigate(`/movies/collections?page=${page}`)}
          />
        </>
      )}
    </section>
  );
}

export function CollectionReader({
  client,
  id,
  imageLoader,
}: {
  client: MediaCollectionsClient;
  id: string;
  imageLoader: ImageLoader;
}) {
  const { t } = useSiteTranslation();
  const { resource, retry } = useResource(`collection:${id}`, (signal) =>
    client.get(id, signal),
  );
  return (
    <section className="flex flex-col gap-6">
      <BackLink href="/movies/collections" />
      {resource.phase === "loading" ? (
        <LoadingContent wall />
      ) : resource.phase === "failed" ? (
        <RequestError status={resource.status} retry={retry} />
      ) : (
        <>
          <header className="flex flex-col gap-3">
            <h1 className="site-heading">{resource.value.title}</h1>
            <p className="text-sm text-muted-foreground">
              {t("collectionCount", { count: resource.value.items.length })}
            </p>
            {resource.value.description && (
              <p className="whitespace-pre-wrap leading-relaxed">
                {resource.value.description}
              </p>
            )}
          </header>
          {resource.value.items.length === 0 ? (
            <EmptyContentView title={t("noEntries")} />
          ) : (
            <div className="poster-grid">
              {resource.value.items.map((entry, index) => (
                <PosterCard
                  key={entry.id}
                  item={entry}
                  imageLoader={imageLoader}
                  priority={index < 4}
                />
              ))}
            </div>
          )}
          <Title title={`${resource.value.title} · ${t("brand")}`} />
        </>
      )}
    </section>
  );
}

function Title({ title }: { title: string }) {
  useEffect(() => {
    document.title = title;
  }, [title]);
  return null;
}
