import { browserImageLoader } from "@assemble/media/browser-client";
import type { MediaImagePhase } from "@assemble/media/components/media-image";
import type {
  ImageLoader,
  MediaCandidate,
  MediaEntry,
  MediaImage,
  MediaImageVariant,
} from "@assemble/media/model";
import { Button } from "@assemble/ui/button";
import { cn } from "@assemble/ui/lib/utils";
import { LoadingState } from "@assemble/ui/loading-state";
import { FilmIcon, RotateCwIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { browserImageSource, posterImageSources } from "../lib/browser-image";
import { useSiteTranslation } from "../lib/i18n";
import { catalogPath, entryPath } from "../lib/navigation";
import { SiteLink } from "./common";

export function MediaImageView({
  image,
  imageLoader,
  alt,
  variant = "360",
  className = "",
  priority = false,
  allowRetry = true,
  responsivePoster = false,
  onStateChange,
}: {
  image: MediaImage | null;
  imageLoader: ImageLoader;
  alt: string;
  variant?: MediaImageVariant;
  className?: string;
  priority?: boolean;
  allowRetry?: boolean;
  responsivePoster?: boolean;
  compact?: boolean;
  onStateChange?: (phase: MediaImagePhase) => void;
}) {
  const { t } = useSiteTranslation();
  const [revision, setRevision] = useState(0);
  const [result, setResult] = useState<{
    key: string;
    source: string | null;
    loader: ImageLoader;
  } | null>(null);
  const key = `${image?.assetId}:${variant}:${revision}`;
  const loaded =
    result?.key === key && result.loader === imageLoader ? result : null;
  // 浏览器资产只有固定同源路径，直接让 HTML 图片元素启动请求。
  const source =
    loaded?.source === null
      ? null
      : imageLoader === browserImageLoader && image
        ? browserImageSource(image, variant)
        : loaded?.source;
  const phase: MediaImagePhase = !image
    ? "missing"
    : source === undefined
      ? "loading"
      : source === null
        ? "failed"
        : "ready";
  useEffect(() => onStateChange?.(phase), [onStateChange, phase]);
  useEffect(() => {
    if (!image || imageLoader === browserImageLoader) return;
    const controller = new AbortController();
    void imageLoader(image, variant, controller.signal).then(
      (source) => {
        if (!controller.signal.aborted)
          setResult({ key, source, loader: imageLoader });
      },
      () => {
        if (!controller.signal.aborted)
          setResult({ key, source: null, loader: imageLoader });
      },
    );
    return () => controller.abort();
  }, [image, imageLoader, key, variant]);
  if (!image)
    return (
      <div
        className={cn("poster-placeholder", className)}
        role="img"
        aria-label={t("noPoster")}
      >
        <FilmIcon aria-hidden="true" />
        <span>{t("noPoster")}</span>
      </div>
    );
  if (source === undefined)
    return (
      <LoadingState
        className={cn("media-image", className)}
        label={t("loading")}
      />
    );
  if (source === null)
    return (
      <div className={cn("poster-placeholder", className)}>
        <span>{t("imageFailed")}</span>
        {allowRetry && (
          <Button
            variant="ghost"
            size="default"
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              setRevision((value) => value + 1);
            }}
            aria-label={t("imageRetry")}
          >
            <RotateCwIcon />
            {t("retry")}
          </Button>
        )}
      </div>
    );
  const imageElement = (
    <img
      className={cn("media-image", className)}
      src={source}
      width={image.width}
      height={image.height}
      alt={alt}
      loading={priority ? "eager" : "lazy"}
      decoding="async"
      fetchPriority={priority ? "high" : "low"}
      onError={() => setResult({ key, source: null, loader: imageLoader })}
    />
  );
  if (responsivePoster && imageLoader === browserImageLoader)
    return (
      <picture>
        {posterImageSources.map(({ media, variant: candidate }) => (
          <source
            key={media}
            media={media}
            srcSet={browserImageSource(image, candidate) ?? undefined}
          />
        ))}
        {imageElement}
      </picture>
    );
  return imageElement;
}

export function PosterCard({
  item,
  imageLoader,
  priority = false,
  returnTo,
}: {
  item: MediaEntry | MediaCandidate;
  imageLoader: ImageLoader;
  priority?: boolean;
  returnTo?: string;
}) {
  const isEntry = "id" in item;
  const destination = isEntry
    ? entryPath(item.scope, item.id)
    : catalogPath(item.identity);
  const href = returnTo
    ? `${destination}?from=${encodeURIComponent(returnTo)}`
    : destination;
  return (
    <article className="poster-card">
      <MediaImageView
        image={item.poster}
        imageLoader={imageLoader}
        alt=""
        priority={priority}
        responsivePoster
        className="poster-art"
      />
      <div className="poster-caption">
        <div className="poster-heading">
          <h3 className="poster-title">
            <SiteLink href={href} title={item.title}>
              {item.title}
            </SiteLink>
          </h3>
          {item.year !== null && (
            <span className="poster-year">{item.year}</span>
          )}
        </div>
        {item.overview?.trim() && (
          <p className="poster-overview">{item.overview}</p>
        )}
      </div>
    </article>
  );
}
