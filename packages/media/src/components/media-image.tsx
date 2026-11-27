import { Button } from "@assemble/ui/button";
import { cn } from "@assemble/ui/lib/utils";
import { LoadingState } from "@assemble/ui/loading-state";
import { ImageOffIcon, RotateCwIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { useMediaTranslation } from "../i18n";
import type {
  MediaImage as Image,
  ImageLoader,
  MediaImageVariant,
} from "../model";
import "./media-image.css";

export type MediaImagePhase = "loading" | "ready" | "missing" | "failed";
export interface MediaImageProps {
  image: Image | null;
  imageLoader: ImageLoader;
  alt: string;
  variant?: MediaImageVariant;
  className?: string;
  priority?: boolean;
  allowRetry?: boolean;
  compact?: boolean;
  onStateChange?: (phase: MediaImagePhase) => void;
}

export function MediaImage({
  image,
  imageLoader,
  alt,
  variant = "360",
  className,
  priority = false,
  allowRetry = true,
  compact = false,
  onStateChange,
}: MediaImageProps) {
  const { t } = useMediaTranslation();
  const [attempt, setAttempt] = useState(0);
  const [result, setResult] = useState<{
    key: string;
    loader: ImageLoader;
    url: string | null;
  } | null>(null);
  const key = `${image?.assetId}:${variant}:${attempt}`;
  const current =
    result?.key === key && result.loader === imageLoader ? result : null;
  const phase: MediaImagePhase = !image
    ? "missing"
    : !current
      ? "loading"
      : current.url
        ? "ready"
        : "failed";
  useEffect(() => onStateChange?.(phase), [onStateChange, phase]);
  useEffect(() => {
    if (!image) return;
    const controller = new AbortController();
    void imageLoader(image, variant, controller.signal).then(
      (url) => {
        if (!controller.signal.aborted)
          setResult({ key, loader: imageLoader, url });
      },
      () => {
        if (!controller.signal.aborted)
          setResult({ key, loader: imageLoader, url: null });
      },
    );
    return () => controller.abort();
  }, [image, imageLoader, variant, key]);
  const imageElement = image && current?.url && (
    <img
      src={current.url}
      alt={alt}
      width={image.width}
      height={image.height}
      loading={priority ? "eager" : "lazy"}
      decoding="async"
      className={variant === "original" ? className : "media-image-content"}
      onError={() => setResult({ key, loader: imageLoader, url: null })}
    />
  );
  // 原图按固有比例放入预览舞台，其余状态和缩略图共用有界的外框。
  if (variant === "original" && imageElement) return imageElement;
  const label = t(phase === "failed" ? "entry.imageFailed" : "entry.noImage");
  return (
    <div
      className={cn("media-image-frame", className)}
      data-image-state={phase}
      data-compact={compact || undefined}
    >
      {imageElement ||
        (phase === "loading" ? (
          <LoadingState
            className="media-image-loading"
            label={t("common.loading")}
          />
        ) : (
          <div className="media-image-placeholder">
            {phase === "failed" && allowRetry ? (
              <Button
                variant="ghost"
                size="icon"
                className="media-image-retry"
                aria-label={`${t("entry.retryImage")}：${alt || label}`}
                title={t("entry.retryImage")}
                onClick={(event) => {
                  event.preventDefault();
                  event.stopPropagation();
                  setAttempt((value) => value + 1);
                }}
              >
                <RotateCwIcon aria-hidden="true" />
              </Button>
            ) : (
              <ImageOffIcon
                role="img"
                aria-label={`${alt || label}：${label}`}
              />
            )}
            <span className={compact ? "sr-only" : "media-image-label"}>
              {label}
            </span>
          </div>
        ))}
    </div>
  );
}
