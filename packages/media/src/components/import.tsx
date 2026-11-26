import { Button } from "@assemble/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@assemble/ui/dialog";
import { Field, FieldGroup, FieldLabel } from "@assemble/ui/field";
import { Input } from "@assemble/ui/input";
import { Select } from "@assemble/ui/select";
import { useId, useState } from "react";
import type { MediaClient } from "../client";
import { type MediaAccessFailure, useMediaTask } from "../hooks/use-media-task";
import { useMediaTranslation } from "../i18n";
import type {
  ImageLoader,
  MediaCandidate,
  MediaCapabilities,
  MediaEntry,
  MediaIdentity,
  MediaJob,
  MediaProvider,
  MediaType,
} from "../model";
import { MediaEmpty, MediaError, MediaLoading } from "./feedback";
import { isActiveJob, MediaJobStatus, watchMediaJob } from "./job";
import { MediaImage } from "./media-image";

export function MediaImport({
  open = true,
  client,
  capabilities,
  imageLoader,
  onClose,
  onImported,
  onAccessFailure,
}: {
  open?: boolean;
  client: MediaClient;
  capabilities: MediaCapabilities;
  imageLoader: ImageLoader;
  onClose: () => void;
  onImported: (entry: MediaEntry) => void;
  onAccessFailure?: (failure: MediaAccessFailure) => void;
}) {
  const { t } = useMediaTranslation();
  const prefix = useId();
  const [provider, setProvider] = useState<MediaProvider>(
    capabilities.providers.tmdb ? "tmdb" : "douban",
  );
  const [mediaType, setMediaType] = useState<MediaType>("movie");
  const [q, setQ] = useState("");
  const [items, setItems] = useState<MediaCandidate[] | null>(null);
  const [candidate, setCandidate] = useState<MediaCandidate | null>(null);
  const [job, setJob] = useState<MediaJob | null>(null);
  const [attempt, setAttempt] = useState<{
    identity: MediaIdentity;
    requestId: string;
  } | null>(null);
  const search = useMediaTask(onAccessFailure);
  const task = useMediaTask(onAccessFailure);
  const enabled = capabilities.providers.tmdb || capabilities.providers.douban;
  const complete = async (initial: MediaJob, signal: AbortSignal) => {
    const result = await watchMediaJob(client, initial, signal, setJob);
    if (result.status === "completed" && result.entryId)
      return client.get(result.entryId, signal);
    return null;
  };
  function start(input: { identity: MediaIdentity; requestId: string }) {
    setAttempt(input);
    setJob(null);
    void task.run(
      async (signal) =>
        complete(
          await client.import(input.identity, input.requestId, signal),
          signal,
        ),
      (entry) => {
        if (entry) onImported(entry);
      },
    );
  }
  return (
    <Dialog
      open={open}
      onOpenChange={(nextOpen) => {
        if (!nextOpen) {
          task.cancel();
          search.cancel();
          onClose();
        }
      }}
    >
      <DialogContent className="max-h-[90svh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{t("import.title")}</DialogTitle>
          <DialogDescription>{t("import.hint")}</DialogDescription>
        </DialogHeader>
        {!enabled ? (
          <MediaEmpty title={t("import.noProvider")} />
        ) : (
          <form
            className="flex flex-col gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              setItems(null);
              setCandidate(null);
              void search.run(
                (signal) =>
                  client.search(provider, mediaType, q.trim(), signal),
                setItems,
              );
            }}
          >
            <FieldGroup className="sm:grid sm:grid-cols-2">
              <Field>
                <FieldLabel htmlFor={`${prefix}-provider`}>
                  {t("import.provider")}
                </FieldLabel>
                <Select
                  id={`${prefix}-provider`}
                  value={provider}
                  disabled={task.pending}
                  onChange={(event) => {
                    search.cancel();
                    setItems(null);
                    setCandidate(null);
                    setProvider(event.target.value as MediaProvider);
                  }}
                >
                  {(["tmdb", "douban"] as const).map((source) => (
                    <option
                      key={source}
                      value={source}
                      disabled={!capabilities.providers[source]}
                    >
                      {t(`import.${source}`)}
                    </option>
                  ))}
                </Select>
              </Field>
              <Field>
                <FieldLabel htmlFor={`${prefix}-type`}>
                  {t("entry.mediaType")}
                </FieldLabel>
                <Select
                  id={`${prefix}-type`}
                  value={mediaType}
                  disabled={task.pending}
                  onChange={(event) => {
                    search.cancel();
                    setItems(null);
                    setCandidate(null);
                    setMediaType(event.target.value as MediaType);
                  }}
                >
                  <option value="movie">{t("type.movie")}</option>
                  <option value="tv">{t("type.tv")}</option>
                </Select>
              </Field>
              <Field className="sm:col-span-2">
                <FieldLabel htmlFor={`${prefix}-q`}>
                  {t("import.query")}
                </FieldLabel>
                <Input
                  id={`${prefix}-q`}
                  type="search"
                  value={q}
                  required
                  maxLength={300}
                  disabled={task.pending}
                  onChange={(event) => setQ(event.target.value)}
                />
              </Field>
            </FieldGroup>
            <Button
              type="submit"
              className="w-fit"
              disabled={!q.trim() || search.pending || task.pending}
            >
              {t("common.search")}
            </Button>
          </form>
        )}
        <MediaError error={search.error} />
        <MediaError error={task.error} />
        {search.pending ? (
          <MediaLoading />
        ) : items?.length === 0 ? (
          <MediaEmpty title={t("import.none")} />
        ) : (
          items && (
            <ul className="flex flex-col gap-3">
              {items.map((item) => (
                <li
                  key={`${item.identity.provider}:${item.identity.mediaType}:${item.identity.externalId}`}
                  className="flex items-start gap-3"
                >
                  <MediaImage
                    image={item.poster}
                    imageLoader={imageLoader}
                    alt={item.title}
                    variant="200"
                    className="aspect-[2/3] w-16 shrink-0 rounded object-cover"
                  />
                  <div className="flex min-w-0 flex-1 flex-col gap-2">
                    <p className="font-medium">
                      {item.title} {item.year ?? ""}
                    </p>
                    <p className="line-clamp-2 text-sm text-muted-foreground">
                      {item.overview || t("entry.noOverview")}
                    </p>
                    <Button
                      variant="outline"
                      size="default"
                      className="w-fit"
                      disabled={task.pending}
                      onClick={() =>
                        void search.run(
                          (signal) => client.catalog(item.identity, signal),
                          setCandidate,
                        )
                      }
                    >
                      {t("import.candidate")}
                    </Button>
                  </div>
                </li>
              ))}
            </ul>
          )
        )}
        {candidate && (
          <section className="flex flex-col gap-3 rounded-lg border p-4">
            <h3 className="font-medium">
              {candidate.title} {candidate.year ?? ""}
            </h3>
            <p className="text-sm text-muted-foreground">
              {candidate.originalTitle}
            </p>
            <p className="whitespace-pre-wrap text-sm">
              {candidate.overview || t("entry.noOverview")}
            </p>
            <Button
              disabled={task.pending || Boolean(job && isActiveJob(job))}
              onClick={() =>
                start({
                  identity: candidate.identity,
                  requestId: crypto.randomUUID(),
                })
              }
            >
              {t("import.confirm")}
            </Button>
          </section>
        )}
        {job && <MediaJobStatus job={job} />}
        <p className="text-xs text-muted-foreground">
          {t("import.pendingHint")}
        </p>
        {attempt &&
          !task.pending &&
          ((!job && Boolean(task.error)) ||
            job?.status === "failed" ||
            job?.status === "interrupted" ||
            job?.status === "cancelled") && (
            <Button
              variant="outline"
              onClick={() =>
                start({
                  ...attempt,
                  requestId: job ? crypto.randomUUID() : attempt.requestId,
                })
              }
            >
              {t("import.retryJob")}
            </Button>
          )}
        {job &&
          (isActiveJob(job) ||
            (job.status === "completed" && Boolean(task.error))) &&
          !task.pending && (
            <Button
              variant="outline"
              onClick={() =>
                void task.run(
                  (signal) => complete(job, signal),
                  (entry) => {
                    if (entry) onImported(entry);
                  },
                )
              }
            >
              {t("import.resumeJob")}
            </Button>
          )}
      </DialogContent>
    </Dialog>
  );
}
