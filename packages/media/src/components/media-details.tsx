import { Badge } from "@assemble/ui/badge";
import { Button } from "@assemble/ui/button";
import { Separator } from "@assemble/ui/separator";
import { CopyIcon } from "lucide-react";
import { useState } from "react";
import { useMediaTranslation } from "../i18n";
import type { ImageLoader, MediaEntry, MediaPersonIdentity } from "../model";
import { MediaImage } from "./media-image";

export function MediaDetails({
  entry,
  imageLoader,
  onActor,
  onCopyMagnet,
  onSignIn,
}: {
  entry: MediaEntry;
  imageLoader: ImageLoader;
  onActor?: (identity: MediaPersonIdentity, name: string) => void;
  onCopyMagnet?: (entry: MediaEntry) => Promise<void>;
  onSignIn?: () => void;
}) {
  const { t } = useMediaTranslation();
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
  return (
    <article className="flex min-w-0 flex-col gap-5">
      <div className="flex flex-col gap-5 sm:flex-row">
        <MediaImage
          image={entry.poster}
          imageLoader={imageLoader}
          alt={entry.title}
          variant="480"
          className="aspect-[2/3] w-40 shrink-0 rounded-lg object-cover"
        />
        <div className="flex min-w-0 flex-1 flex-col gap-3">
          <h2 className="break-words text-2xl font-semibold">{entry.title}</h2>
          {entry.originalTitle && entry.originalTitle !== entry.title && (
            <p className="text-muted-foreground">{entry.originalTitle}</p>
          )}
          <div className="flex flex-wrap gap-2">
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
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
            <dt className="text-muted-foreground">{t("entry.releaseDate")}</dt>
            <dd>{entry.releaseDate ?? t("common.unknown")}</dd>
            {entry.mediaType === "tv" && (
              <>
                <dt className="text-muted-foreground">{t("entry.episodes")}</dt>
                <dd>{entry.episodeCount ?? t("common.unknown")}</dd>
              </>
            )}
            <dt className="text-muted-foreground">{t("entry.watchedAt")}</dt>
            <dd>{entry.watchedAt ?? t("common.unknown")}</dd>
          </dl>
          {entry.magnetUri && onCopyMagnet ? (
            <Button
              variant="outline"
              className="w-fit"
              onClick={() => void copyMagnet()}
              disabled={copy === "pending"}
            >
              <CopyIcon data-icon="inline-start" />
              {t(copy === "done" ? "entry.copied" : "entry.copyMagnet")}
            </Button>
          ) : entry.magnetAvailable && onSignIn ? (
            <Button variant="outline" className="w-fit" onClick={onSignIn}>
              {t("entry.signInMagnet")}
            </Button>
          ) : (
            <p className="text-sm text-muted-foreground">
              {t(
                entry.magnetAvailable ? "entry.signInMagnet" : "entry.noMagnet",
              )}
            </p>
          )}
          {copy === "error" && (
            <p role="alert" className="text-sm text-destructive">
              {t("entry.copyFailed")}
            </p>
          )}
        </div>
      </div>
      <Separator />
      <section className="flex flex-col gap-2">
        <h3 className="font-medium">{t("entry.overview")}</h3>
        <p className="whitespace-pre-wrap break-words text-sm leading-relaxed">
          {entry.overview || t("entry.noOverview")}
        </p>
      </section>
      <section className="flex flex-col gap-2">
        <h3 className="font-medium">{t("entry.review")}</h3>
        <p className="whitespace-pre-wrap break-words text-sm leading-relaxed">
          {entry.review || t("entry.noReview")}
        </p>
      </section>
      {entry.notes && (
        <section className="flex flex-col gap-2">
          <h3 className="font-medium">{t("entry.notes")}</h3>
          <p className="whitespace-pre-wrap break-words text-sm">
            {entry.notes}
          </p>
        </section>
      )}
      <section className="flex flex-col gap-3">
        <h3 className="font-medium">{t("entry.cast")}</h3>
        {entry.cast.length === 0 ? (
          <p className="text-sm text-muted-foreground">{t("entry.noCast")}</p>
        ) : (
          <ul className="grid grid-cols-2 gap-3 sm:grid-cols-3">
            {entry.cast.map((person) => (
              <li
                key={`${person.identity?.provider}:${person.identity?.externalId ?? person.name}:${person.character}`}
                className="flex min-w-0 items-start gap-2"
              >
                <MediaImage
                  image={person.photo}
                  imageLoader={imageLoader}
                  alt={person.name}
                  variant="200"
                  className="aspect-[2/3] w-12 shrink-0 rounded object-cover"
                />
                <div className="flex min-w-0 flex-col gap-1">
                  {person.identity && onActor ? (
                    <Button
                      variant="link"
                      className="h-auto justify-start whitespace-normal p-0 text-left"
                      onClick={() =>
                        person.identity && onActor(person.identity, person.name)
                      }
                    >
                      {person.name}
                    </Button>
                  ) : (
                    <p className="break-words text-sm">{person.name}</p>
                  )}
                  {person.character && (
                    <p className="text-xs text-muted-foreground">
                      {t("entry.character", { name: person.character })}
                    </p>
                  )}
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>
      {entry.identities.length > 0 && (
        <section className="flex flex-col gap-2">
          <h3 className="font-medium">{t("entry.source")}</h3>
          <ul className="flex flex-wrap gap-2">
            {entry.identities.map((identity) => (
              <li
                key={`${identity.provider}:${identity.mediaType}:${identity.externalId}`}
              >
                <Badge variant="outline">
                  {t(`import.${identity.provider}`)} · {identity.externalId}
                </Badge>
              </li>
            ))}
          </ul>
        </section>
      )}
    </article>
  );
}
