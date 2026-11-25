import {
  type ImageLoader,
  type MediaClient,
  type MediaCollectionsClient,
  MediaDetails,
  type MediaEntry,
} from "@assemble/media";
import { Badge } from "@assemble/ui/badge";
import { Button } from "@assemble/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@assemble/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@assemble/ui/dialog";
import { Separator } from "@assemble/ui/separator";
import { useEffect, useRef, useState } from "react";
import { useSiteTranslation } from "../lib/i18n";
import {
  actorPath,
  loginPath,
  navigate,
  type SiteRoute,
  wallPath,
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
import { MediaImageView, PosterCard } from "./poster";

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
      <BackLink href={wallPath(client.scope)} />
      {resource.phase === "loading" ? (
        <LoadingContent />
      ) : resource.phase === "failed" ? (
        <RequestError status={resource.status} retry={retry} />
      ) : (
        <>
          <h1 className="sr-only">{resource.value.title}</h1>
          <MediaDetails
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
          <Gallery
            client={client}
            entry={resource.value}
            imageLoader={imageLoader}
          />
          <Title title={`${resource.value.title} · ${t("brand")}`} />
        </>
      )}
    </div>
  );
}

function Gallery({
  client,
  entry,
  imageLoader,
}: ReaderProps & { entry: MediaEntry }) {
  const { t } = useSiteTranslation();
  const { resource, retry } = useResource(
    `${client.scope}:${entry.id}:gallery`,
    (signal) => client.gallery(entry.id, signal),
  );
  return (
    <section className="flex flex-col gap-4">
      <Separator />
      <h2 className="text-lg font-semibold">{t("gallery")}</h2>
      {resource.phase === "loading" ? (
        <LoadingContent />
      ) : resource.phase === "failed" ? (
        <RequestError status={resource.status} retry={retry} />
      ) : resource.value.items.length === 0 ? (
        <p className="text-sm text-muted-foreground">{t("galleryEmpty")}</p>
      ) : (
        <div className="gallery-grid">
          {resource.value.items.map((image, index) => {
            const description = t("galleryImage", {
              title: entry.title,
              number: index + 1,
            });
            return (
              <Dialog key={image.assetId}>
                <DialogTrigger asChild>
                  <Button
                    variant="ghost"
                    className="gallery-trigger"
                    aria-label={description}
                  >
                    <MediaImageView
                      image={image}
                      imageLoader={imageLoader}
                      alt=""
                      variant="360"
                      className="gallery-art"
                      allowRetry={false}
                    />
                  </Button>
                </DialogTrigger>
                <DialogContent className="max-w-[min(94dvw,72rem)]">
                  <DialogHeader>
                    <DialogTitle>{entry.title}</DialogTitle>
                    <DialogDescription>{description}</DialogDescription>
                  </DialogHeader>
                  <MediaImageView
                    image={image}
                    imageLoader={imageLoader}
                    alt={description}
                    variant="original"
                    className="gallery-expanded"
                  />
                </DialogContent>
              </Dialog>
            );
          })}
        </div>
      )}
    </section>
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
            <MediaImageView
              image={resource.value.poster}
              imageLoader={imageLoader}
              alt={resource.value.title}
              variant="480"
              className="catalog-poster"
              priority
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
              <p className="text-sm text-muted-foreground">
                {t("catalogHelp")}
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
      <h1 className="site-heading">{t("actors")}</h1>
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
            <h2 className="text-2xl font-semibold">
              {resource.value.actor.name}
            </h2>
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

import { Select } from "@assemble/ui/select";

function ActorType({
  route,
}: {
  route: Extract<SiteRoute, { kind: "actor" }>;
}) {
  const { t } = useSiteTranslation();
  return (
    <Select
      id="actor-type"
      className="max-w-40"
      value={route.mediaType}
      onChange={(event) => {
        const type = event.target.value;
        if (type === "movie" || type === "tv")
          navigate(actorPath(route.identity, type));
      }}
    >
      <option value="movie">{t("movie")}</option>
      <option value="tv">{t("tv")}</option>
    </Select>
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
            <EmptyContentView
              title={t("collectionsEmpty")}
              description={t("collectionsHelp")}
            />
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
