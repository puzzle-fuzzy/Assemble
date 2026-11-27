import { Alert, AlertDescription } from "@assemble/ui/alert";
import { Button } from "@assemble/ui/button";
import { Checkbox } from "@assemble/ui/checkbox";
import { DatePicker } from "@assemble/ui/date-picker";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@assemble/ui/dialog";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
} from "@assemble/ui/field";
import { Input } from "@assemble/ui/input";
import { parseCalendarDate } from "@assemble/ui/lib/date-input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@assemble/ui/select";
import { Textarea } from "@assemble/ui/textarea";
import { useId, useRef, useState } from "react";
import type { MediaClient } from "../client";
import {
  type MediaAccessFailure,
  mediaErrorCode,
  useMediaTask,
} from "../hooks/use-media-task";
import { useMediaTranslation } from "../i18n";
import type { MediaContent, MediaEntry } from "../model";
import { MediaError } from "./feedback";

export function entryContent(entry?: MediaEntry): MediaContent {
  return {
    published: entry?.published ?? false,
    mediaType: entry?.mediaType ?? "movie",
    title: entry?.title ?? "",
    originalTitle: entry?.originalTitle ?? "",
    year: entry?.year ?? null,
    releaseDate: entry?.releaseDate ?? null,
    overview: entry?.overview ?? "",
    genres: entry?.genres ?? [],
    episodeCount: entry?.episodeCount ?? null,
    status: entry?.status ?? "planned",
    watchedAt: entry?.watchedAt ?? null,
    review: entry?.review ?? null,
    notes: entry?.notes ?? null,
    magnetUri: entry?.magnetUri ?? null,
  };
}
export function validMediaContent(content: MediaContent) {
  return (
    Boolean(content.title.trim()) &&
    content.title.length <= 1000 &&
    (content.year === null ||
      (Number.isInteger(content.year) &&
        content.year >= 1800 &&
        content.year <= 9999)) &&
    (content.episodeCount === null ||
      (Number.isInteger(content.episodeCount) && content.episodeCount > 0)) &&
    (!content.magnetUri ||
      /^magnet:\?xt=urn:btih:[a-zA-Z0-9]+(?:&|$)/i.test(content.magnetUri)) &&
    [content.releaseDate, content.watchedAt].every(
      (value) => !value || Boolean(parseCalendarDate(value)),
    )
  );
}
export function MediaEditor({
  client,
  entry,
  onClose,
  onSaved,
  onAccessFailure,
  viewerAccountId,
}: {
  client: MediaClient;
  entry?: MediaEntry;
  onClose: () => void;
  onSaved: (entry: MediaEntry) => void;
  onAccessFailure?: (failure: MediaAccessFailure) => void;
  viewerAccountId?: string;
}) {
  const { t } = useMediaTranslation();
  const prefix = useId();
  const [base, setBase] = useState(entry);
  const [content, setContent] = useState(() => entryContent(entry));
  const [genres, setGenres] = useState(entry?.genres.join(", ") ?? "");
  const [invalid, setInvalid] = useState(false);
  const task = useMediaTask(onAccessFailure);
  const requestId = useRef(crypto.randomUUID());
  const set = <K extends keyof MediaContent>(
    key: K,
    value: MediaContent[K],
  ) => {
    setContent((previous) => ({ ...previous, [key]: value }));
    setInvalid(false);
  };
  const conflict = mediaErrorCode(task.error) === "VERSION_CONFLICT";
  const ownerCanPublish = base
    ? Boolean(viewerAccountId && base.owner?.id === viewerAccountId)
    : client.scope === "private";
  const publicationLocked = !ownerCanPublish && !base?.published;
  const fields = [
    ["title", "text"],
    ["originalTitle", "text"],
    ["year", "number"],
    ["releaseDate", "date"],
    ["episodeCount", "number"],
    ["watchedAt", "date"],
  ] as const;
  const areas = ["overview", "review", "notes", "magnetUri"] as const;
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !task.pending) onClose();
      }}
    >
      <DialogContent
        className="max-h-[90svh] overflow-y-auto sm:max-w-2xl"
        onEscapeKeyDown={(event) => {
          if (task.pending) event.preventDefault();
        }}
        onInteractOutside={(event) => {
          if (task.pending) event.preventDefault();
        }}
      >
        <DialogHeader>
          <DialogTitle>
            {t(base ? "entry.editTitle" : "entry.createTitle")}
          </DialogTitle>
          <DialogDescription>
            {t(`scope.${base?.scope ?? client.scope}Hint`)}
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-5"
          onSubmit={(event) => {
            event.preventDefault();
            const value = {
              ...content,
              title: content.title.trim(),
              genres: [
                ...new Set(
                  genres
                    .split(/[,，]/)
                    .map((item) => item.trim())
                    .filter(Boolean),
                ),
              ],
            };
            if (!validMediaContent(value)) {
              setInvalid(true);
              return;
            }
            void task.run(
              (signal) =>
                base
                  ? client.update(base.id, base.version, value, signal)
                  : client.create(value, requestId.current, signal),
              onSaved,
            );
          }}
        >
          <FieldGroup>
            <FieldGroup className="sm:grid sm:grid-cols-2">
              <Field>
                <FieldLabel htmlFor={`${prefix}-type`}>
                  {t("entry.mediaType")}
                </FieldLabel>
                <Select
                  value={content.mediaType}
                  disabled={task.pending}
                  onValueChange={(value) =>
                    set("mediaType", value as MediaContent["mediaType"])
                  }
                >
                  <SelectTrigger id={`${prefix}-type`} className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      <SelectItem value="movie">{t("type.movie")}</SelectItem>
                      <SelectItem value="tv">{t("type.tv")}</SelectItem>
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
              <Field>
                <FieldLabel htmlFor={`${prefix}-status`}>
                  {t("entry.status")}
                </FieldLabel>
                <Select
                  value={content.status}
                  disabled={task.pending}
                  onValueChange={(value) =>
                    set("status", value as MediaContent["status"])
                  }
                >
                  <SelectTrigger id={`${prefix}-status`} className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      {(["planned", "watching", "completed"] as const).map(
                        (status) => (
                          <SelectItem key={status} value={status}>
                            {t(`status.${status}`)}
                          </SelectItem>
                        ),
                      )}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
              {fields.map(([key, type]) => (
                <Field
                  key={key}
                  data-invalid={
                    invalid && key === "title" && !content.title.trim()
                  }
                >
                  <FieldLabel htmlFor={`${prefix}-${key}`}>
                    {t(`entry.${key === "episodeCount" ? "episodes" : key}`)}
                  </FieldLabel>
                  {type === "date" ? (
                    <DatePicker
                      id={`${prefix}-${key}`}
                      label={t(`entry.${key}`)}
                      value={content[key] ?? ""}
                      disabled={task.pending}
                      onChange={(value) => set(key, value || null)}
                    />
                  ) : (
                    <Input
                      id={`${prefix}-${key}`}
                      type={type}
                      value={content[key] ?? ""}
                      required={key === "title"}
                      maxLength={
                        key === "title" || key === "originalTitle"
                          ? 1000
                          : undefined
                      }
                      min={
                        key === "year"
                          ? 1800
                          : key === "episodeCount"
                            ? 1
                            : undefined
                      }
                      max={key === "year" ? 9999 : undefined}
                      disabled={task.pending}
                      aria-invalid={
                        invalid && key === "title" && !content.title.trim()
                      }
                      onChange={(event) =>
                        set(
                          key,
                          type === "number"
                            ? event.target.value === ""
                              ? null
                              : Number(event.target.value)
                            : key === "title" || key === "originalTitle"
                              ? event.target.value
                              : event.target.value || null,
                        )
                      }
                    />
                  )}
                </Field>
              ))}
            </FieldGroup>
            <Field>
              <FieldLabel htmlFor={`${prefix}-genres`}>
                {t("entry.genres")}
              </FieldLabel>
              <Input
                id={`${prefix}-genres`}
                value={genres}
                disabled={task.pending}
                onChange={(event) => setGenres(event.target.value)}
              />
              <FieldDescription>{t("entry.genresHint")}</FieldDescription>
            </Field>
            {(base?.scope ?? client.scope) === "private" && (
              <Field>
                <Field orientation="horizontal">
                  <Checkbox
                    id={`${prefix}-published`}
                    checked={content.published}
                    disabled={task.pending || publicationLocked}
                    onCheckedChange={(checked) =>
                      !publicationLocked && set("published", checked === true)
                    }
                    aria-describedby={`${prefix}-published-hint`}
                  />
                  <FieldLabel htmlFor={`${prefix}-published`}>
                    {t("publication.publish")}
                  </FieldLabel>
                </Field>
                <FieldDescription id={`${prefix}-published-hint`}>
                  {t(
                    publicationLocked
                      ? "publication.ownerOnly"
                      : "publication.hint",
                  )}
                </FieldDescription>
              </Field>
            )}
            {areas.map((key) => (
              <Field key={key}>
                <FieldLabel htmlFor={`${prefix}-${key}`}>
                  {t(`entry.${key === "magnetUri" ? "magnet" : key}`)}
                </FieldLabel>
                <Textarea
                  id={`${prefix}-${key}`}
                  value={content[key] ?? ""}
                  maxLength={key === "magnetUri" ? 16000 : 20000}
                  rows={key === "overview" ? 4 : 3}
                  disabled={task.pending}
                  onChange={(event) =>
                    set(
                      key,
                      key === "overview"
                        ? event.target.value
                        : event.target.value || null,
                    )
                  }
                />
                {key === "notes" && (
                  <FieldDescription>{t("entry.notesHint")}</FieldDescription>
                )}
              </Field>
            ))}
          </FieldGroup>
          {invalid && <FieldError>{t("entry.validation")}</FieldError>}
          {conflict ? (
            <Alert>
              <AlertDescription>
                <p>{t("entry.conflict")}</p>
                <Button
                  type="button"
                  variant="outline"
                  disabled={task.pending}
                  onClick={() => {
                    if (base)
                      void task.run(
                        (signal) => client.get(base.id, signal),
                        (latest) => {
                          setBase(latest);
                          setContent(entryContent(latest));
                          setGenres(latest.genres.join(", "));
                        },
                      );
                  }}
                >
                  {t("entry.reload")}
                </Button>
              </AlertDescription>
            </Alert>
          ) : (
            <MediaError error={task.error} />
          )}
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={task.pending}
              onClick={onClose}
            >
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={task.pending || conflict}>
              {t(task.pending ? "common.loading" : "common.save")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
