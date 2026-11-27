import { Alert, AlertDescription } from "@assemble/ui/alert";
import { Badge } from "@assemble/ui/badge";
import { Button } from "@assemble/ui/button";
import { Separator } from "@assemble/ui/separator";
import { CopyIcon } from "lucide-react";
import { useState } from "react";
import type { MediaClient } from "../client";
import type { MediaAccessFailure } from "../hooks/use-media-task";
import { useMediaTranslation } from "../i18n";
import type { ImageLoader, MediaEntry, MediaPersonIdentity } from "../model";
import { MediaEntryPosters } from "./entry-posters";
import { MediaImage } from "./media-image";

export function MediaDetails({
  entry,
  imageLoader,
  onActor,
  onCopyMagnet,
  onSignIn,
  presentation = "workspace",
  galleryClient,
  onAccessFailure,
}: {
  entry: MediaEntry;
  imageLoader: ImageLoader;
  onActor?: (identity: MediaPersonIdentity, name: string) => void;
  onCopyMagnet?: (entry: MediaEntry) => Promise<void>;
  onSignIn?: () => void;
  presentation?: "workspace" | "reader" | "management";
  galleryClient?: MediaClient;
  onAccessFailure?: (failure: MediaAccessFailure) => void;
}) {
  const { t } = useMediaTranslation();
  const reading = presentation !== "workspace";
  const Heading = presentation === "reader" ? "h1" : "h2";
  const SectionHeading = presentation === "reader" ? "h2" : "h3";
  const [copy, setCopy] = useState<"idle" | "pending" | "done" | "error">(
    "idle",
  );
  async function copyMagnet() {
    if (!entry.magnetUri || !onCopyMagnet) return;
    setCopy("pending");
    try {
      await onCopyMagnet(entry);
      setCopy("done");
    } catch {
      setCopy("error");
    }
  }
  const details = (
    <>
      {!reading && <Separator />}
      {(!reading || entry.overview.trim()) && (
        <section className={reading ? "detail-section" : "flex flex-col gap-2"}>
          <SectionHeading className="font-medium">
            {t("entry.overview")}
          </SectionHeading>
          <p
            className={
              reading
                ? "detail-prose"
                : "whitespace-pre-wrap break-words text-sm leading-relaxed"
            }
          >
            {entry.overview || t("entry.noOverview")}
          </p>
        </section>
      )}
      {(!reading || entry.review?.trim()) && (
        <section className={reading ? "detail-section" : "flex flex-col gap-2"}>
          <SectionHeading className="font-medium">
            {t("entry.review")}
          </SectionHeading>
          <p
            className={
              reading
                ? "detail-prose"
                : "whitespace-pre-wrap break-words text-sm leading-relaxed"
            }
          >
            {entry.review || t("entry.noReview")}
          </p>
        </section>
      )}
      {entry.notes && (
        <section className={reading ? "detail-section" : "flex flex-col gap-2"}>
          <SectionHeading className="font-medium">
            {t("entry.notes")}
          </SectionHeading>
          <p
            className={
              reading
                ? "detail-prose"
                : "whitespace-pre-wrap break-words text-sm"
            }
          >
            {entry.notes}
          </p>
        </section>
      )}
      {(!reading || entry.cast.length > 0) && (
        <section className={reading ? "detail-section" : "flex flex-col gap-3"}>
          <SectionHeading className="font-medium">
            {t("entry.cast")}
          </SectionHeading>
          {entry.cast.length === 0 ? (
            <p className="text-sm text-muted-foreground">{t("entry.noCast")}</p>
          ) : (
            <ul
              className={
                reading
                  ? "detail-cast-list"
                  : "grid grid-cols-2 gap-3 sm:grid-cols-3"
              }
            >
              {entry.cast.map((person) => (
                <li
                  key={`${person.identity?.provider}:${person.identity?.externalId ?? person.name}:${person.character}`}
                  className="detail-actor-item"
                >
                  <MediaImage
                    image={person.photo}
                    imageLoader={imageLoader}
                    alt={person.name}
                    variant="200"
                    compact
                    className="detail-actor-photo"
                  />
                  <div className="detail-actor-description">
                    {person.identity && onActor ? (
                      <Button
                        variant="link"
                        className="detail-actor-name"
                        onClick={() =>
                          person.identity &&
                          onActor(person.identity, person.name)
                        }
                      >
                        {person.name}
                      </Button>
                    ) : (
                      <p className="detail-actor-name">{person.name}</p>
                    )}
                    {person.character && (
                      <p className="detail-actor-role">
                        {t("entry.character", { name: person.character })}
                      </p>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}
      {entry.identities.length > 0 && (
        <section className={reading ? "detail-section" : "flex flex-col gap-2"}>
          <SectionHeading className="font-medium">
            {t("entry.source")}
          </SectionHeading>
          <ul className="flex flex-wrap gap-2">
            {entry.identities.map((identity) => (
              <li
                key={`${identity.provider}:${identity.mediaType}:${identity.externalId}`}
              >
                {reading ? (
                  <a
                    className="text-sm underline underline-offset-4"
                    href={
                      identity.provider === "tmdb"
                        ? `https://www.themoviedb.org/${identity.mediaType}/${encodeURIComponent(identity.externalId)}`
                        : `https://movie.douban.com/subject/${encodeURIComponent(identity.externalId)}/`
                    }
                    target="_blank"
                    rel="noreferrer"
                  >
                    {t(`import.${identity.provider}`)}
                  </a>
                ) : (
                  <Badge variant="outline">
                    {t(`import.${identity.provider}`)} · {identity.externalId}
                  </Badge>
                )}
              </li>
            ))}
          </ul>
        </section>
      )}
    </>
  );
  return (
    <article
      className={
        reading
          ? "mx-auto flex w-full max-w-6xl min-w-0 flex-col gap-8"
          : "flex min-w-0 flex-col gap-5"
      }
    >
      <div
        className={
          reading ? "detail-viewer" : "flex flex-col gap-5 sm:flex-row"
        }
        data-presentation={presentation}
      >
        {presentation === "management" && galleryClient ? (
          <aside
            className="detail-poster-column"
            aria-label={t("entry.gallery")}
          >
            <MediaEntryPosters
              key={`${galleryClient.scope}:${entry.id}:${entry.version}`}
              client={galleryClient}
              entry={entry}
              imageLoader={imageLoader}
              onAccessFailure={onAccessFailure}
            />
          </aside>
        ) : (
          <MediaImage
            image={entry.poster}
            imageLoader={imageLoader}
            alt={entry.title}
            variant="480"
            className={
              reading
                ? "aspect-[2/3] w-48 shrink-0 rounded-lg object-cover sm:w-60"
                : "aspect-[2/3] w-40 shrink-0 rounded-lg object-cover"
            }
          />
        )}
        <div
          className={
            reading ? "detail-content" : "flex min-w-0 flex-1 flex-col gap-3"
          }
        >
          <Heading
            className={
              reading ? "detail-title" : "break-words text-2xl font-semibold"
            }
          >
            {entry.title}
          </Heading>
          {entry.originalTitle && entry.originalTitle !== entry.title && (
            <p className="detail-original-title">{entry.originalTitle}</p>
          )}
          <div className={reading ? "detail-labels" : "flex flex-wrap gap-2"}>
            <Badge>{t(`status.${entry.status}`)}</Badge>
            <Badge variant="secondary">{t(`type.${entry.mediaType}`)}</Badge>
            {entry.year !== null && (
              <Badge variant="outline">{entry.year}</Badge>
            )}
            {entry.genres.map((genre) => (
              <Badge key={genre} variant="outline">
                {genre}
              </Badge>
            ))}
          </div>
          <dl className="mt-5 grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
            {(!reading || entry.releaseDate) && (
              <>
                <dt className="text-muted-foreground">
                  {t("entry.releaseDate")}
                </dt>
                <dd>{entry.releaseDate ?? t("common.unknown")}</dd>
              </>
            )}
            {entry.mediaType === "tv" &&
              (!reading || entry.episodeCount !== null) && (
                <>
                  <dt className="text-muted-foreground">
                    {t("entry.episodes")}
                  </dt>
                  <dd>{entry.episodeCount ?? t("common.unknown")}</dd>
                </>
              )}
            {(!reading || entry.watchedAt) && (
              <>
                <dt className="text-muted-foreground">
                  {t("entry.watchedAt")}
                </dt>
                <dd>{entry.watchedAt ?? t("common.unknown")}</dd>
              </>
            )}
          </dl>
          {entry.magnetUri && onCopyMagnet ? (
            <Button
              variant="outline"
              className="mt-4 w-fit"
              onClick={() => void copyMagnet()}
              disabled={copy === "pending"}
            >
              <CopyIcon data-icon="inline-start" />
              {t(copy === "done" ? "entry.copied" : "entry.copyMagnet")}
            </Button>
          ) : entry.magnetAvailable && onSignIn ? (
            <Button variant="outline" className="mt-4 w-fit" onClick={onSignIn}>
              {t("entry.signInMagnet")}
            </Button>
          ) : (
            (!reading || entry.magnetAvailable) && (
              <p className="text-sm text-muted-foreground">
                {t(
                  entry.magnetAvailable
                    ? "entry.signInMagnet"
                    : "entry.noMagnet",
                )}
              </p>
            )
          )}
          {copy === "error" && (
            <Alert variant="destructive">
              <AlertDescription>{t("entry.copyFailed")}</AlertDescription>
            </Alert>
          )}
          {reading && <div>{details}</div>}
        </div>
      </div>
      {!reading && details}
    </article>
  );
}
