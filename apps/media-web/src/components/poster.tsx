import type {
  ImageLoader,
  MediaCandidate,
  MediaEntry,
  MediaImage,
  MediaImageVariant,
} from "@assemble/media";
import { Badge } from "@assemble/ui/badge";
import { Button } from "@assemble/ui/button";
import { cn } from "@assemble/ui/lib/utils";
import { Skeleton } from "@assemble/ui/skeleton";
import { FilmIcon, RotateCwIcon } from "lucide-react";
import { useEffect, useState } from "react";
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
}: {
  image: MediaImage | null;
  imageLoader: ImageLoader;
  alt: string;
  variant?: MediaImageVariant;
  className?: string;
  priority?: boolean;
  allowRetry?: boolean;
}) {
  const { t } = useSiteTranslation();
  const [revision, setRevision] = useState(0);
  const [result, setResult] = useState<{
    key: string;
    source: string | null;
  } | null>(null);
  const key = `${image?.assetId}:${variant}:${revision}`;
  const source = result?.key === key ? result.source : undefined;
  useEffect(() => {
    if (!image) return;
    const controller = new AbortController();
    void imageLoader(image, variant, controller.signal).then(
      (source) => {
        if (!controller.signal.aborted) setResult({ key, source });
      },
      () => {
        if (!controller.signal.aborted) setResult({ key, source: null });
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
      <Skeleton
        className={cn("media-image", className)}
        aria-label={t("loading")}
      />
    );
  if (source === null)
    return (
      <div className={cn("poster-placeholder", className)}>
        <span>{t("imageFailed")}</span>
        {allowRetry && (
          <Button
            variant="ghost"
            size="sm"
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
  return (
    <img
      className={cn("media-image", className)}
      src={source}
      width={image.width}
      height={image.height}
      alt={alt}
      loading={priority ? "eager" : "lazy"}
      decoding="async"
      fetchPriority={priority ? "high" : "auto"}
      onError={() => setResult({ key, source: null })}
    />
  );
}

export function PosterCard({
  item,
  imageLoader,
  priority = false,
}: {
  item: MediaEntry | MediaCandidate;
  imageLoader: ImageLoader;
  priority?: boolean;
}) {
  const { t } = useSiteTranslation();
  const isEntry = "id" in item;
  const href = isEntry
    ? entryPath(item.scope, item.id)
    : catalogPath(item.identity);
  return (
    <article className="poster-card">
      <MediaImageView
        image={item.poster}
        imageLoader={imageLoader}
        alt=""
        priority={priority}
        className="poster-art"
      />
      <div className="flex flex-col items-start gap-1.5">
        <h2 className="poster-title">
          <SiteLink href={href}>{item.title}</SiteLink>
        </h2>
        <p className="text-sm text-muted-foreground">
          {item.year ?? t("unknownYear")} ·{" "}
          {t(isEntry ? item.mediaType : item.identity.mediaType)}
        </p>
        {isEntry && <Badge variant="secondary">{t(item.status)}</Badge>}
      </div>
    </article>
  );
}
