import { Button } from "@assemble/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@assemble/ui/dialog";
import { useEffect, useRef, useState } from "react";
import type { MediaClient } from "../client";
import { type MediaAccessFailure, useMediaTask } from "../hooks/use-media-task";
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
  const task = useMediaTask(onAccessFailure);
  const syncAttempt = useRef<{ version: number; requestId: string } | null>(
    null,
  );
  const load = () =>
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
        onUpdated(latest);
      },
    );
  useEffect(() => {
    void task.run((signal) => client.gallery(entry.id, signal), setGallery);
  }, [client, entry.id, task.run]);
  function sync() {
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
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent className="max-h-[90svh] overflow-y-auto sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>
            {t("entry.gallery")} · {entry.title}
          </DialogTitle>
          <DialogDescription>{t("entry.galleryHint")}</DialogDescription>
        </DialogHeader>
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" disabled={task.pending} onClick={load}>
            {t("common.refresh")}
          </Button>
          <Button
            disabled={task.pending || current.identities.length === 0}
            onClick={sync}
          >
            {t("entry.syncGallery")}
          </Button>
        </div>
        <MediaError error={task.error} retry={load} />
        {job && <MediaJobStatus job={job} />}
        <p className="text-xs text-muted-foreground">
          {t("import.pendingHint")}
        </p>
        {!gallery && task.pending ? (
          <MediaLoading />
        ) : gallery?.items.length === 0 ? (
          <MediaEmpty title={t("entry.noGallery")} />
        ) : (
          <ul className="grid grid-cols-2 gap-4 sm:grid-cols-3">
            {gallery?.items.map((image) => (
              <li key={image.assetId} className="flex flex-col gap-2">
                <MediaImage
                  image={image}
                  imageLoader={imageLoader}
                  alt={entry.title}
                  className="aspect-[2/3] w-full rounded object-cover"
                />
                <Button
                  variant="outline"
                  size="default"
                  disabled={
                    task.pending || image.assetId === current.poster?.assetId
                  }
                  onClick={() =>
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
                          value ? { ...value, version: latest.version } : null,
                        );
                        syncAttempt.current = null;
                        onUpdated(latest);
                      },
                    )
                  }
                >
                  {t(
                    image.assetId === current.poster?.assetId
                      ? "entry.currentPoster"
                      : "entry.selectPoster",
                  )}
                </Button>
              </li>
            ))}
          </ul>
        )}
      </DialogContent>
    </Dialog>
  );
}
