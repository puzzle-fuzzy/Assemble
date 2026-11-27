import { Button } from "@assemble/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@assemble/ui/dialog";
import { cn } from "@assemble/ui/lib/utils";
import { CheckIcon, RefreshCwIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { MediaClient } from "../client";
import {
  type MediaAccessFailure,
  mediaErrorCode,
  useMediaTask,
} from "../hooks/use-media-task";
import { useMediaTranslation } from "../i18n";
import type { ImageLoader, MediaEntry, MediaGallery, MediaJob } from "../model";
import { MediaEmpty, MediaError, MediaLoading } from "./feedback";
import { MediaJobStatus, watchMediaJob } from "./job";
import { MediaImage } from "./media-image";

export function MediaGalleryEditor({
  client,
  entry,
  imageLoader,
  onClose,
  onUpdated,
  onAccessFailure,
}: {
  client: MediaClient;
  entry: MediaEntry;
  imageLoader: ImageLoader;
  onClose: () => void;
  onUpdated: (entry: MediaEntry) => void;
  onAccessFailure?: (failure: MediaAccessFailure) => void;
}) {
  const { t } = useMediaTranslation();
  const [gallery, setGallery] = useState<MediaGallery | null>(null);
  const [job, setJob] = useState<MediaJob | null>(null);
  const [current, setCurrent] = useState(entry);
  const [operation, setOperation] = useState<"load" | "sync" | "poster">(
    "load",
  );
  const task = useMediaTask(onAccessFailure);
  const syncAttempt = useRef<{ version: number; requestId: string } | null>(
    null,
  );
  const load = () => {
    setOperation("load");
    void task.run(
      async (signal) => {
        const latest = await client.get(entry.id, signal);
        const images = await client.gallery(entry.id, signal);
        return { latest, images };
      },
      ({ latest, images }) => {
        syncAttempt.current = null;
        setCurrent(latest);
        setGallery(images);
        setJob(null);
        onUpdated(latest);
      },
    );
  };
  useEffect(() => {
    void task.run(
      async (signal) => {
        const latest = await client.get(entry.id, signal);
        const images = await client.gallery(entry.id, signal);
        return { latest, images };
      },
      ({ latest, images }) => {
        setCurrent(latest);
        setGallery(images);
      },
    );
  }, [client, entry.id, task.run]);
  function sync() {
    setOperation("sync");
    const attempt = syncAttempt.current ?? {
      version: current.version,
      requestId: crypto.randomUUID(),
    };
    syncAttempt.current = attempt;
    void task.run(
      async (signal) => {
        const initial = await client.syncGallery(
          entry.id,
          attempt.version,
          attempt.requestId,
          signal,
        );
        const result = await watchMediaJob(client, initial, signal, setJob);
        if (result.status !== "completed") return null;
        const latest = await client.get(entry.id, signal);
        const images = await client.gallery(entry.id, signal);
        return { latest, images };
      },
      (result) => {
        syncAttempt.current = null;
        if (result) {
          setCurrent(result.latest);
          onUpdated(result.latest);
          setGallery(result.images);
        }
      },
    );
  }
  const needsReload =
    job?.errorCode === "VERSION_CONFLICT" ||
    Boolean(task.error && mediaErrorCode(task.error) === "VERSION_CONFLICT");
  const syncing = task.pending && operation === "sync";
  const singleImage = gallery?.items.length === 1;
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent
        className={cn(
          "flex max-h-[90svh] min-w-0 flex-col gap-4",
          singleImage ? "sm:max-w-sm" : "sm:max-w-3xl",
        )}
      >
        <DialogHeader>
          <DialogTitle className="pr-8 leading-6 break-words">
            {entry.title}
          </DialogTitle>
          <DialogDescription className="sr-only">
            {t("entry.gallery")}
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <span className="text-sm text-muted-foreground">
            {gallery
              ? t("entry.galleryCount", { count: gallery.items.length })
              : t("entry.gallery")}
          </span>
          <div className="flex flex-wrap gap-2">
            {needsReload && (
              <Button
                variant="outline"
                size="sm"
                disabled={task.pending}
                onClick={load}
              >
                {t("entry.reload")}
              </Button>
            )}
            <Button
              size="sm"
              title={t(
                current.identities.length === 0
                  ? "error.MEDIA_SOURCE_REQUIRED"
                  : "entry.syncGalleryLimit",
              )}
              disabled={
                task.pending || current.identities.length === 0 || needsReload
              }
              onClick={sync}
            >
              <RefreshCwIcon
                data-icon="inline-start"
                aria-hidden="true"
                className={syncing ? "motion-safe:animate-spin" : undefined}
              />
              {t(
                syncing
                  ? "entry.syncingGallery"
                  : job?.status === "failed" ||
                      (task.error && syncAttempt.current)
                    ? "entry.retryGallery"
                    : "entry.syncGallery",
              )}
            </Button>
          </div>
        </div>
        <MediaError
          error={task.error}
          retry={needsReload || syncAttempt.current ? undefined : load}
        />
        {job && <MediaJobStatus job={job} compact />}
        <div className="min-h-0 overflow-y-auto overscroll-contain p-0.5">
          {!gallery && task.pending ? (
            <MediaLoading />
          ) : gallery?.items.length === 0 ? (
            <MediaEmpty title={t("entry.noGallery")} />
          ) : (
            <ul
              className={cn(
                "grid gap-3",
                singleImage
                  ? "mx-auto w-44 grid-cols-1"
                  : "grid-cols-[repeat(auto-fill,minmax(min(100%,7rem),1fr))]",
              )}
            >
              {gallery?.items.map((image, index) => (
                <li key={image.assetId} className="flex flex-col gap-2">
                  <MediaImage
                    image={image}
                    imageLoader={imageLoader}
                    alt={t("entry.galleryImage", {
                      title: entry.title,
                      number: index + 1,
                    })}
                    className="aspect-[2/3] w-full rounded-md object-cover"
                    compact
                  />
                  {image.assetId === current.poster?.assetId ? (
                    <span className="flex min-h-8 items-center justify-center gap-1 text-xs font-medium">
                      <CheckIcon className="size-3.5" aria-hidden="true" />
                      {t("entry.currentPoster")}
                    </span>
                  ) : (
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={
                        task.pending ||
                        image.assetId === current.poster?.assetId
                      }
                      onClick={() => {
                        setOperation("poster");
                        void task.run(
                          (signal) =>
                            client.selectPoster(
                              current.id,
                              gallery.version,
                              image.assetId,
                              signal,
                            ),
                          (latest) => {
                            setCurrent(latest);
                            setGallery((value) =>
                              value
                                ? { ...value, version: latest.version }
                                : null,
                            );
                            syncAttempt.current = null;
                            onUpdated(latest);
                          },
                        );
                      }}
                    >
                      {t("entry.selectPoster")}
                    </Button>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
