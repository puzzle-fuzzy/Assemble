import { Button } from "@assemble/ui/button";
import { LoadingState } from "@assemble/ui/loading-state";
import { ImageOffIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { useMediaTranslation } from "../i18n";
import type {
  MediaImage as Image,
  ImageLoader,
  MediaImageVariant,
} from "../model";

export function MediaImage({
  image,
  imageLoader,
  alt,
  variant = "360",
  className,
}: {
  image: Image | null;
  imageLoader: ImageLoader;
  alt: string;
  variant?: MediaImageVariant;
  className?: string;
}) {
  const { t } = useMediaTranslation();
  const [state, setState] = useState<{
    key: string;
    url: string | null;
    failed: boolean;
  }>({ key: "", url: null, failed: false });
  const [attempt, setAttempt] = useState(0);
  const key = image ? `${image.assetId}:${variant}:${attempt}` : "";
  useEffect(() => {
    const controller = new AbortController();
    setState({ key, url: null, failed: false });
    if (image)
      void imageLoader(image, variant, controller.signal).then(
        (url) => {
          if (!controller.signal.aborted) setState({ key, url, failed: false });
        },
        () => {
          if (!controller.signal.aborted)
            setState({ key, url: null, failed: true });
        },
      );
    return () => controller.abort();
  }, [image, imageLoader, variant, key]);
  if (image && (state.key !== key || (!state.url && !state.failed)))
    return <LoadingState className={className} label={t("common.loading")} />;
  if (state.url && state.key === key && !state.failed)
    return (
      <img
        src={state.url}
        alt={alt}
        width={image?.width}
        height={image?.height}
        loading="lazy"
        decoding="async"
        className={className}
        onError={() => setState({ key, url: null, failed: true })}
      />
    );
  return (
    <div className={className}>
      <div className="flex h-full min-h-24 flex-col items-center justify-center gap-2 rounded-lg bg-muted p-3 text-center text-muted-foreground">
        <ImageOffIcon aria-hidden="true" />
        <span className="text-xs">
          {t(state.failed ? "entry.imageFailed" : "entry.noImage")}
        </span>
        {state.failed && (
          <Button
            variant="outline"
            size="default"
            onClick={() => setAttempt((value) => value + 1)}
          >
            {t("common.retry")}
          </Button>
        )}
      </div>
    </div>
  );
}
