import { useEffect, useState } from "react";
import type { MediaClient } from "../client";
import { type MediaAccessFailure, useMediaTask } from "../hooks/use-media-task";
import { useMediaTranslation } from "../i18n";
import type { ImageLoader, MediaEntry, MediaImage } from "../model";
import { MediaError } from "./feedback";
import { MediaPosterViewer } from "./poster-viewer";

export function MediaEntryPosters({
  client,
  entry,
  imageLoader,
  onAccessFailure,
}: {
  client: MediaClient;
  entry: MediaEntry;
  imageLoader: ImageLoader;
  onAccessFailure?: (failure: MediaAccessFailure) => void;
}) {
  const { t } = useMediaTranslation();
  const [images, setImages] = useState<MediaImage[]>([]);
  const { run, pending, error } = useMediaTask(onAccessFailure);
  function load() {
    void run(
      (signal) => client.gallery(entry.id, signal),
      (gallery) => setImages(gallery.items),
    );
  }
  useEffect(() => {
    void run(
      (signal) => client.gallery(entry.id, signal),
      (gallery) => setImages(gallery.items),
    );
  }, [client, entry.id, run]);
  return (
    <>
      <MediaPosterViewer
        image={entry.poster}
        images={images}
        title={entry.title}
        imageLoader={imageLoader}
      />
      {pending && (
        <p className="detail-gallery-feedback" role="status">
          {t("common.loading")}
        </p>
      )}
      <MediaError error={error} retry={load} />
    </>
  );
}
