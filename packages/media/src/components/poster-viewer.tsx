import { Button } from "@assemble/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@assemble/ui/dialog";
import { ArrowLeftIcon, ArrowRightIcon, ExpandIcon } from "lucide-react";
import { type ComponentType, useEffect, useRef, useState } from "react";
import { useMediaTranslation } from "../i18n";
import type { MediaImage as Image, ImageLoader } from "../model";
import {
  MediaImage,
  type MediaImagePhase,
  type MediaImageProps,
} from "./media-image";
import { createPosterMotion } from "./poster-motion";
import "./detail-viewer.css";

export function MediaPosterViewer({
  image,
  images = [],
  title,
  imageLoader,
  sessionSignal,
  ImageComponent = MediaImage,
}: {
  image: Image | null;
  images?: Image[];
  title: string;
  imageLoader: ImageLoader;
  sessionSignal?: AbortSignal;
  ImageComponent?: ComponentType<MediaImageProps>;
}) {
  const { t: mediaText } = useMediaTranslation();
  const t = (key: string, options: Record<string, unknown> = {}) =>
    mediaText(`entry.${key}`, options);
  const [coverState, setCoverState] = useState<MediaImagePhase>("loading");
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
        <ImageComponent
          image={image}
          imageLoader={imageLoader}
          alt={title}
          variant="480"
          className="detail-cover"
          priority
          onStateChange={setCoverState}
        />
        {image && coverState !== "failed" && (
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
              <ImageComponent
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
                    <ImageComponent
                      image={poster}
                      imageLoader={imageLoader}
                      alt=""
                      variant="200"
                      allowRetry={false}
                      compact
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
