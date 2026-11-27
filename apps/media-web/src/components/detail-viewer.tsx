import { useTranslation } from "@assemble/display";
import type {
  ImageLoader,
  MediaClient,
  MediaEntry,
  MediaPersonIdentity,
} from "@assemble/media";
import { MediaPosterViewer } from "@assemble/media/components/poster-viewer";
import { Badge } from "@assemble/ui/badge";
import { Button } from "@assemble/ui/button";
import { CopyIcon } from "lucide-react";
import { type ReactNode, useState } from "react";
import { useSiteTranslation } from "../lib/i18n";
import { useResource } from "../lib/use-resource";
import { RequestError } from "./common";
import { MediaImageView } from "./poster";

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

export function PosterViewer(
  props: Omit<Parameters<typeof MediaPosterViewer>[0], "ImageComponent">,
) {
  return <MediaPosterViewer {...props} ImageComponent={MediaImageView} />;
}
