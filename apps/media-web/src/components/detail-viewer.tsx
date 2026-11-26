import { useTranslation } from "@assemble/display";
import type {
  ImageLoader,
  MediaClient,
  MediaEntry,
  MediaImage,
  MediaPersonIdentity,
} from "@assemble/media";
import { Badge } from "@assemble/ui/badge";
import { Button } from "@assemble/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@assemble/ui/dialog";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  CopyIcon,
  ExpandIcon,
} from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { useSiteTranslation } from "../lib/i18n";
import { useResource } from "../lib/use-resource";
import { RequestError } from "./common";
import { MediaImageView } from "./poster";
import { createPosterMotion } from "./poster-motion";

export function DetailViewer({
  client,
  entry,
  imageLoader,
  onActor,
  onCopyMagnet,
  onSignIn,
  sessionSignal,
}: {
  client: MediaClient;
  entry: MediaEntry;
  imageLoader: ImageLoader;
  onActor: (identity: MediaPersonIdentity) => void;
  onCopyMagnet?: (entry: MediaEntry) => Promise<void>;
  onSignIn?: () => void;
  sessionSignal: AbortSignal;
}) {
  const { t } = useSiteTranslation();
  const { t: mediaText } = useTranslation("media");
  const gallery = useResource(`${client.scope}:${entry.id}:gallery`, (signal) =>
    client.gallery(entry.id, signal),
  );
  const [copy, setCopy] = useState<"idle" | "pending" | "done" | "error">(
    "idle",
  );
  async function copyMagnet() {
    if (copy === "pending" || !onCopyMagnet) return;
    setCopy("pending");
    try {
      await onCopyMagnet(entry);
      setCopy("done");
    } catch {
      setCopy("error");
    }
  }
  return (
    <article className="detail-viewer">
      <aside className="detail-poster-column" aria-label={t("gallery")}>
        <PosterViewer
          image={entry.poster}
          images={
            gallery.resource.phase === "ready"
              ? gallery.resource.value.items
              : []
          }
          title={entry.title}
          imageLoader={imageLoader}
          sessionSignal={sessionSignal}
        />
        {gallery.resource.phase === "loading" && (
          <p className="detail-gallery-feedback" role="status">
            {t("loading")}
          </p>
        )}
        {gallery.resource.phase === "failed" && (
          <RequestError
            status={gallery.resource.status}
            retry={gallery.retry}
          />
        )}
      </aside>
      <div className="detail-content">
        <header className="detail-introduction">
          <h1>{entry.title}</h1>
          <div className="detail-labels">
            <Badge variant="secondary">{t(entry.mediaType)}</Badge>
            <Badge variant="outline">{t(entry.status)}</Badge>
            {entry.genres.map((genre) => (
              <Badge key={genre} variant="outline">
                {genre}
              </Badge>
            ))}
          </div>
          {entry.originalTitle && entry.originalTitle !== entry.title && (
            <p className="detail-original-title">{entry.originalTitle}</p>
          )}
          <dl className="detail-facts">
            {(entry.releaseDate || entry.year !== null) && (
              <div>
                <dt>
                  {mediaText(
                    entry.releaseDate ? "entry.releaseDate" : "entry.year",
                  )}
                </dt>
                <dd>{entry.releaseDate ?? entry.year}</dd>
              </div>
            )}
            {entry.mediaType === "tv" && entry.episodeCount !== null && (
              <div>
                <dt>{mediaText("entry.episodes")}</dt>
                <dd>{entry.episodeCount}</dd>
              </div>
            )}
            {entry.identities.length > 0 && (
              <div>
                <dt>{t("source")}</dt>
                <dd className="detail-source-links">
                  {entry.identities.map((identity) => (
                    <a
                      key={`${identity.provider}:${identity.externalId}`}
                      href={
                        identity.provider === "tmdb"
                          ? `https://www.themoviedb.org/${identity.mediaType}/${encodeURIComponent(identity.externalId)}`
                          : `https://movie.douban.com/subject/${encodeURIComponent(identity.externalId)}/`
                      }
                      target="_blank"
                      rel="noreferrer"
                    >
                      {mediaText(`import.${identity.provider}`)}
                    </a>
                  ))}
                </dd>
              </div>
            )}
            {entry.watchedAt && (
              <div>
                <dt>{mediaText("entry.watchedAt")}</dt>
                <dd>{entry.watchedAt}</dd>
              </div>
            )}
          </dl>
        </header>
        {entry.overview.trim() && (
          <DetailSection title={t("overview")}>
            <p className="detail-prose">{entry.overview}</p>
          </DetailSection>
        )}
        {entry.review?.trim() && (
          <DetailSection title={mediaText("entry.review")}>
            <p className="detail-prose">{entry.review}</p>
          </DetailSection>
        )}
        {entry.notes?.trim() && (
          <DetailSection title={mediaText("entry.notes")}>
            <p className="detail-prose">{entry.notes}</p>
          </DetailSection>
        )}
        {entry.magnetAvailable && (
          <DetailSection title={mediaText("entry.magnet")}>
            {entry.magnetUri && onCopyMagnet ? (
              <Button
                variant="outline"
                onClick={() => void copyMagnet()}
                disabled={copy === "pending"}
              >
                <CopyIcon data-icon="inline-start" />
                {mediaText(
                  copy === "done" ? "entry.copied" : "entry.copyMagnet",
                )}
              </Button>
            ) : onSignIn ? (
              <Button variant="outline" onClick={onSignIn}>
                {mediaText("entry.signInMagnet")}
              </Button>
            ) : null}
            <p
              className="detail-copy-feedback"
              role={copy === "error" ? "alert" : "status"}
              aria-live="polite"
            >
              {copy === "error"
                ? mediaText("entry.copyFailed")
                : copy === "done"
                  ? t("copied")
                  : ""}
            </p>
          </DetailSection>
        )}
        {entry.cast.length > 0 && (
          <DetailSection title={mediaText("entry.cast")}>
            <ul className="detail-cast-list">
              {entry.cast.map((person) => (
                <li
                  key={`${person.identity?.provider}:${person.identity?.externalId ?? person.name}:${person.character}`}
                >
                  <div className="detail-actor-item">
                    <div
                      className="detail-actor-photo"
                      role="img"
                      aria-label={person.name}
                    >
                      <MediaImageView
                        image={person.photo}
                        imageLoader={imageLoader}
                        alt=""
                        variant="200"
                        className="detail-actor-image"
                        allowRetry={false}
                      />
                    </div>
                    <div className="detail-actor-description">
                      {person.identity ? (
                        <Button
                          variant="link"
                          className="detail-actor-name"
                          onClick={() =>
                            person.identity && onActor(person.identity)
                          }
                        >
                          {person.name}
                        </Button>
                      ) : (
                        <p className="detail-actor-name">{person.name}</p>
                      )}
                      {person.character && (
                        <p className="detail-actor-role">
                          {mediaText("entry.character", {
                            name: person.character,
                          })}
                        </p>
                      )}
                    </div>
                  </div>
                </li>
              ))}
            </ul>
          </DetailSection>
        )}
      </div>
    </article>
  );
}

function DetailSection({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <section className="detail-section">
      <h2>{title}</h2>
      {children}
    </section>
  );
}

export function PosterViewer({
  image,
  images = [],
  title,
  imageLoader,
  sessionSignal,
}: {
  image: MediaImage | null;
  images?: MediaImage[];
  title: string;
  imageLoader: ImageLoader;
  sessionSignal?: AbortSignal;
}) {
  const { t } = useSiteTranslation();
  const [open, setOpen] = useState(false);
  const [selectedAssetId, setSelectedAssetId] = useState<string | null>(null);
  const returnFocus = useRef<HTMLElement | null>(null);
  const startTouch = useRef<number | null>(null);
  const thumbnails = useRef<HTMLUListElement | null>(null);
  const posterFrame = useRef<HTMLDivElement | null>(null);
  const preview = useRef<HTMLDivElement | null>(null);
  const motion = useRef<ReturnType<typeof createPosterMotion> | null>(null);
  useEffect(() => () => motion.current?.cancel(), []);
  useEffect(() => {
    // 对话框通过 portal 脱离了隐藏的页面，重新核验会话时必须单独关掉。
    const close = () => {
      motion.current?.cancel();
      setOpen(false);
    };
    sessionSignal?.addEventListener("abort", close, { once: true });
    if (sessionSignal?.aborted) close();
    return () => sessionSignal?.removeEventListener("abort", close);
  }, [sessionSignal]);
  const posters = [
    ...(image && !images.some((item) => item.assetId === image.assetId)
      ? [image]
      : []),
    ...images,
  ];
  // 图集可能晚于封面返回；按资产而不是下标选中，避免预览悄悄换图。
  const current =
    posters.find((poster) => poster.assetId === selectedAssetId) ?? posters[0];
  const currentIndex = current ? posters.indexOf(current) : 0;
  useEffect(() => {
    if (!open) return;
    thumbnails.current
      ?.querySelector<HTMLButtonElement>(
        `button[data-poster-position="${currentIndex}"]`,
      )
      ?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [open, currentIndex]);
  function show(trigger: HTMLElement) {
    if (sessionSignal?.aborted) return;
    returnFocus.current = trigger;
    motion.current?.cancel();
    motion.current = createPosterMotion(
      posterFrame.current?.querySelector<HTMLImageElement>("img") ?? null,
    );
    setSelectedAssetId(image?.assetId ?? posters[0]?.assetId ?? null);
    setOpen(true);
  }
  function choose(position: number) {
    if (motion.current && !motion.current.interrupt()) return;
    if (posters.length)
      setSelectedAssetId(
        posters[(position + posters.length) % posters.length]?.assetId ?? null,
      );
  }
  return (
    <div className="detail-poster-view">
      <div className="detail-poster-frame" ref={posterFrame}>
        <MediaImageView
          image={image}
          imageLoader={imageLoader}
          alt={title}
          variant="480"
          className="detail-cover"
          priority
          allowRetry={false}
        />
        {image && (
          <Button
            variant="ghost"
            className="detail-poster-trigger"
            aria-label={t("previewPoster", { title })}
            aria-haspopup="dialog"
            onClick={(event) => show(event.currentTarget)}
          >
            <span className="detail-poster-hint">
              <ExpandIcon aria-hidden="true" />
            </span>
          </Button>
        )}
      </div>
      {posters.length > 1 && images.length > 0 && (
        <Button
          variant="outline"
          className="detail-gallery-open"
          onClick={(event) => show(event.currentTarget)}
          aria-haspopup="dialog"
        >
          {t("browsePosters", { count: posters.length })}
        </Button>
      )}
      <Dialog
        open={open && Boolean(current)}
        onOpenChange={(next) => {
          if (next) setOpen(true);
          else if (motion.current)
            motion.current.close(preview.current, () => setOpen(false));
          else setOpen(false);
        }}
      >
        <DialogContent
          className="detail-lightbox"
          data-single-image={posters.length === 1 ? "true" : undefined}
          ref={(content) => {
            preview.current = content;
            if (content) motion.current?.open(content);
          }}
          onCloseAutoFocus={(event) => {
            motion.current?.cancel();
            event.preventDefault();
            returnFocus.current?.focus({ preventScroll: true });
          }}
          onKeyDown={(event) => {
            if (
              event.altKey ||
              event.ctrlKey ||
              event.metaKey ||
              posters.length < 2
            )
              return;
            if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
              event.preventDefault();
              choose(currentIndex + (event.key === "ArrowLeft" ? -1 : 1));
            } else if (event.key === "Home" || event.key === "End") {
              event.preventDefault();
              choose(event.key === "Home" ? 0 : posters.length - 1);
            }
          }}
        >
          <DialogHeader
            className={`detail-lightbox-heading${posters.length === 1 ? " sr-only" : ""}`}
          >
            <DialogTitle>{title}</DialogTitle>
            <DialogDescription>
              {posters.length > 1
                ? t("imagePosition", {
                    number: currentIndex + 1,
                    count: posters.length,
                  })
                : t("previewPoster", { title })}
            </DialogDescription>
          </DialogHeader>
          <div
            className="detail-lightbox-stage"
            onTouchStart={(event) => {
              startTouch.current = event.touches[0]?.clientX ?? null;
            }}
            onTouchEnd={(event) => {
              const start = startTouch.current;
              startTouch.current = null;
              const end = event.changedTouches[0]?.clientX;
              if (
                start !== null &&
                end !== undefined &&
                Math.abs(start - end) > 48
              )
                choose(currentIndex + (start > end ? 1 : -1));
            }}
          >
            {current && (
              <MediaImageView
                key={current.assetId}
                image={current}
                imageLoader={imageLoader}
                alt={t("galleryImage", { title, number: currentIndex + 1 })}
                variant="original"
                className="detail-lightbox-image"
                priority
              />
            )}
          </div>
          {posters.length > 1 && (
            <div className="detail-lightbox-controls">
              <Button
                variant="outline"
                disabled={posters.length < 2}
                aria-label={t("previousImage")}
                onClick={() => choose(currentIndex - 1)}
              >
                <ArrowLeftIcon />
                {t("previousImage")}
              </Button>
              <p role="status" aria-live="polite" aria-atomic="true">
                {currentIndex + 1} / {posters.length}
              </p>
              <Button
                variant="outline"
                disabled={posters.length < 2}
                aria-label={t("nextImage")}
                onClick={() => choose(currentIndex + 1)}
              >
                {t("nextImage")}
                <ArrowRightIcon />
              </Button>
            </div>
          )}
          {posters.length > 1 && (
            <ul className="detail-lightbox-thumbnails" ref={thumbnails}>
              {posters.map((poster, position) => (
                <li key={poster.assetId}>
                  <Button
                    variant="ghost"
                    aria-label={t("chooseImage", { number: position + 1 })}
                    aria-pressed={position === currentIndex}
                    data-poster-position={position}
                    onClick={() => choose(position)}
                  >
                    <MediaImageView
                      image={poster}
                      imageLoader={imageLoader}
                      alt=""
                      variant="200"
                      allowRetry={false}
                    />
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}
