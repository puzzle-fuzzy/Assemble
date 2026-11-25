import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@assemble/ui/alert-dialog";
import { Badge } from "@assemble/ui/badge";
import { Button } from "@assemble/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@assemble/ui/card";
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
import {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@assemble/ui/table";
import {
  ArrowLeftIcon,
  ImageIcon,
  PencilIcon,
  PlusIcon,
  RefreshCwIcon,
  SearchIcon,
  TrashIcon,
} from "lucide-react";
import { useEffect, useId, useState } from "react";
import type { MediaClient } from "../client";
import { type MediaAccessFailure, useMediaTask } from "../hooks/use-media-task";
import { useMediaTranslation } from "../i18n";
import type {
  ImageLoader,
  MediaActorCredits,
  MediaCapabilities,
  MediaEntry,
  MediaList,
  MediaPersonIdentity,
  MediaQuery,
} from "../model";
import { MediaEditor } from "./editor";
import {
  MediaEmpty,
  MediaError,
  MediaLoading,
  MediaPagination,
} from "./feedback";
import { MediaGalleryEditor } from "./gallery";
import { MediaImport } from "./import";
import { MediaDetails } from "./media-details";
import { MediaImage } from "./media-image";

export interface MediaWorkspaceProps {
  client: MediaClient;
  imageLoader: ImageLoader;
  canEdit: boolean;
  accountKey: string;
  onAccessFailure?: (failure: MediaAccessFailure) => void;
  onCopyMagnet?: (entry: MediaEntry) => Promise<void>;
  onSignIn?: () => void;
}
export function MediaWorkspace(props: MediaWorkspaceProps) {
  return (
    <WorkspaceContent
      key={`${props.accountKey}:${props.client.scope}`}
      {...props}
    />
  );
}
function WorkspaceContent({
  client,
  imageLoader,
  canEdit,
  onAccessFailure,
  onCopyMagnet,
  onSignIn,
}: MediaWorkspaceProps) {
  const { t } = useMediaTranslation();
  const prefix = useId();
  const [query, setQuery] = useState<MediaQuery>({
    page: 1,
    pageSize: 20,
    sort: "updated",
  });
  const [q, setQ] = useState("");
  const [genre, setGenre] = useState("");
  const [year, setYear] = useState("");
  const [list, setList] = useState<MediaList | null>(null);
  const [capabilities, setCapabilities] = useState<MediaCapabilities | null>(
    null,
  );
  const [selected, setSelected] = useState<MediaEntry | null>(null);
  const [editor, setEditor] = useState<MediaEntry | "new" | null>(null);
  const [importing, setImporting] = useState(false);
  const [gallery, setGallery] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [actor, setActor] = useState<{
    identity: MediaPersonIdentity;
    name: string;
  } | null>(null);
  const [credits, setCredits] = useState<MediaActorCredits | null>(null);
  const [revision, setRevision] = useState(0);
  const listTask = useMediaTask(onAccessFailure);
  const detailTask = useMediaTask(onAccessFailure);
  const actorTask = useMediaTask(onAccessFailure);
  const mutation = useMediaTask(onAccessFailure);
  useEffect(() => {
    detailTask.cancel();
    setSelected(null);
    void query;
  }, [query, detailTask.cancel]);
  useEffect(() => {
    void revision;
    detailTask.cancel();
    setList(null);
    void listTask.run(
      async (signal) => {
        const result = await client.list(query, signal);
        const caps = await client.capabilities(signal);
        return { result, caps };
      },
      ({ result, caps }) => {
        setList(result);
        setCapabilities(caps);
      },
    );
  }, [client, query, revision, listTask.run, detailTask.cancel]);
  const allowed =
    canEdit &&
    Boolean(
      capabilities &&
        (client.scope === "shared"
          ? capabilities.canMaintainShared
          : capabilities.canMaintainPrivate),
    );
  function saved(entry: MediaEntry) {
    setSelected(entry);
    setEditor(null);
    setImporting(false);
    setRevision((value) => value + 1);
  }
  function openEntry(id: string) {
    setSelected(null);
    void detailTask.run((signal) => client.get(id, signal), setSelected);
  }
  function showActor(identity: MediaPersonIdentity, name: string) {
    setActor({ identity, name });
    setCredits(null);
    void actorTask.run(
      (signal) =>
        client.actor(identity, selected?.mediaType ?? "movie", signal),
      setCredits,
    );
  }
  return (
    <div className="@container/media flex min-w-0 flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle>{t(`scope.${client.scope}`)}</CardTitle>
          <CardDescription>{t(`scope.${client.scope}Hint`)}</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex flex-wrap gap-2">
              {list &&
                (["all", "planned", "watching", "completed"] as const).map(
                  (status) => (
                    <Badge variant="secondary" key={status}>
                      {t("list.count", {
                        status: t(
                          status === "all" ? "list.all" : `status.${status}`,
                        ),
                        count: list.counts[status],
                      })}
                    </Badge>
                  ),
                )}
            </div>
            <div className="flex flex-wrap gap-2">
              <Button
                variant="outline"
                size="sm"
                disabled={listTask.pending}
                onClick={() => {
                  detailTask.cancel();
                  setSelected(null);
                  setRevision((value) => value + 1);
                }}
              >
                <RefreshCwIcon data-icon="inline-start" />
                {t("common.refresh")}
              </Button>
              {allowed && (
                <>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setEditor("new")}
                  >
                    <PlusIcon data-icon="inline-start" />
                    {t("common.create")}
                  </Button>
                  <Button size="sm" onClick={() => setImporting(true)}>
                    <SearchIcon data-icon="inline-start" />
                    {t("common.import")}
                  </Button>
                </>
              )}
            </div>
          </div>
          <form
            className="flex flex-col gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              setSelected(null);
              setQuery((previous) => ({
                ...previous,
                q: q.trim() || undefined,
                genre: genre.trim() || undefined,
                year: year ? Number(year) : undefined,
                page: 1,
              }));
            }}
          >
            <FieldGroup className="grid grid-cols-2 @xl/media:grid-cols-4 @5xl/media:grid-cols-6">
              <Field className="col-span-2">
                <FieldLabel htmlFor={`${prefix}-q`}>
                  {t("list.query")}
                </FieldLabel>
                <Input
                  id={`${prefix}-q`}
                  type="search"
                  placeholder={t("list.placeholder")}
                  value={q}
                  maxLength={300}
                  onChange={(event) => setQ(event.target.value)}
                />
              </Field>
              <Field>
                <FieldLabel htmlFor={`${prefix}-type`}>
                  {t("entry.mediaType")}
                </FieldLabel>
                <Select
                  id={`${prefix}-type`}
                  value={query.mediaType ?? "all"}
                  onChange={(event) => {
                    setSelected(null);
                    setQuery((previous) => ({
                      ...previous,
                      mediaType:
                        event.target.value === "all"
                          ? undefined
                          : (event.target.value as MediaQuery["mediaType"]),
                      page: 1,
                    }));
                  }}
                >
                  <option value="all">{t("type.all")}</option>
                  <option value="movie">{t("type.movie")}</option>
                  <option value="tv">{t("type.tv")}</option>
                </Select>
              </Field>
              <Field>
                <FieldLabel htmlFor={`${prefix}-status`}>
                  {t("entry.status")}
                </FieldLabel>
                <Select
                  id={`${prefix}-status`}
                  value={query.status ?? "all"}
                  onChange={(event) => {
                    setSelected(null);
                    setQuery((previous) => ({
                      ...previous,
                      status:
                        event.target.value === "all"
                          ? undefined
                          : (event.target.value as MediaQuery["status"]),
                      page: 1,
                    }));
                  }}
                >
                  <option value="all">{t("status.all")}</option>
                  {(["planned", "watching", "completed"] as const).map(
                    (status) => (
                      <option key={status} value={status}>
                        {t(`status.${status}`)}
                      </option>
                    ),
                  )}
                </Select>
              </Field>
              <Field>
                <FieldLabel htmlFor={`${prefix}-genre`}>
                  {t("list.genre")}
                </FieldLabel>
                <Input
                  id={`${prefix}-genre`}
                  value={genre}
                  maxLength={100}
                  onChange={(event) => setGenre(event.target.value)}
                />
              </Field>
              <Field>
                <FieldLabel htmlFor={`${prefix}-year`}>
                  {t("list.year")}
                </FieldLabel>
                <Input
                  id={`${prefix}-year`}
                  type="number"
                  min={1800}
                  max={9999}
                  value={year}
                  onChange={(event) => setYear(event.target.value)}
                />
              </Field>
              <Field>
                <FieldLabel htmlFor={`${prefix}-sort`}>
                  {t("list.sort")}
                </FieldLabel>
                <Select
                  id={`${prefix}-sort`}
                  value={query.sort}
                  onChange={(event) =>
                    setQuery((previous) => ({
                      ...previous,
                      sort: event.target.value as MediaQuery["sort"],
                      page: 1,
                    }))
                  }
                >
                  {(["updated", "title", "year", "watched"] as const).map(
                    (sort) => (
                      <option key={sort} value={sort}>
                        {t(`list.${sort === "year" ? "yearSort" : sort}`)}
                      </option>
                    ),
                  )}
                </Select>
              </Field>
            </FieldGroup>
            <div className="flex flex-wrap gap-2">
              <Button type="submit" size="sm">
                <SearchIcon data-icon="inline-start" />
                {t("common.search")}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                type="button"
                onClick={() => {
                  setQ("");
                  setGenre("");
                  setYear("");
                  setSelected(null);
                  setQuery({ page: 1, pageSize: 20, sort: "updated" });
                }}
              >
                {t("list.reset")}
              </Button>
            </div>
          </form>
        </CardContent>
      </Card>
      <MediaError
        error={listTask.error}
        retry={() => setRevision((value) => value + 1)}
      />
      <MediaError error={detailTask.error} />
      {selected ? (
        <Card>
          <CardHeader>
            <div className="flex flex-wrap justify-between gap-2">
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setSelected(null)}
              >
                <ArrowLeftIcon data-icon="inline-start" />
                {t("common.back")}
              </Button>
              {allowed && (
                <div className="flex flex-wrap gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setEditor(selected)}
                  >
                    <PencilIcon data-icon="inline-start" />
                    {t("common.edit")}
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setGallery(true)}
                  >
                    <ImageIcon data-icon="inline-start" />
                    {t("entry.gallery")}
                  </Button>
                  <Button
                    variant="destructive"
                    size="sm"
                    onClick={() => setRemoving(true)}
                  >
                    <TrashIcon data-icon="inline-start" />
                    {t("common.remove")}
                  </Button>
                </div>
              )}
            </div>
          </CardHeader>
          <CardContent>
            <MediaDetails
              key={`${selected.id}:${selected.version}`}
              entry={selected}
              imageLoader={imageLoader}
              onActor={showActor}
              onCopyMagnet={onCopyMagnet}
              onSignIn={onSignIn}
            />
          </CardContent>
        </Card>
      ) : listTask.pending || detailTask.pending ? (
        <MediaLoading />
      ) : list && list.items.length === 0 ? (
        <MediaEmpty
          title={t(
            query.q ||
              query.mediaType ||
              query.status ||
              query.genre ||
              query.year
              ? "common.noMatches"
              : "common.empty",
          )}
          description={
            query.q ||
            query.mediaType ||
            query.status ||
            query.genre ||
            query.year
              ? t("common.noMatchesHint")
              : allowed
                ? t("common.emptyHint")
                : undefined
          }
        />
      ) : (
        list && (
          <Card>
            <CardContent>
              <Table>
                <TableCaption>{t("list.showing")}</TableCaption>
                <TableHeader>
                  <TableRow>
                    <TableHead>{t("entry.title")}</TableHead>
                    <TableHead>{t("entry.mediaType")}</TableHead>
                    <TableHead>{t("entry.year")}</TableHead>
                    <TableHead>{t("entry.status")}</TableHead>
                    <TableHead>{t("entry.watchedAt")}</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {list.items.map((entry) => (
                    <TableRow key={entry.id}>
                      <TableCell>
                        <Button
                          variant="link"
                          className="h-auto max-w-80 justify-start whitespace-normal p-0 text-left"
                          aria-label={t("common.details", {
                            title: entry.title,
                          })}
                          onClick={() => openEntry(entry.id)}
                        >
                          {entry.title}
                        </Button>
                        {entry.originalTitle &&
                          entry.originalTitle !== entry.title && (
                            <p className="max-w-80 truncate text-xs text-muted-foreground">
                              {entry.originalTitle}
                            </p>
                          )}
                      </TableCell>
                      <TableCell>{t(`type.${entry.mediaType}`)}</TableCell>
                      <TableCell>{entry.year ?? t("common.unknown")}</TableCell>
                      <TableCell>
                        <Badge variant="secondary">
                          {t(`status.${entry.status}`)}
                        </Badge>
                      </TableCell>
                      <TableCell>
                        {entry.watchedAt ?? t("common.unknown")}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        )
      )}
      {!selected && list && (
        <MediaPagination
          total={list.total}
          page={list.page}
          pageSize={list.pageSize}
          pending={listTask.pending}
          onPage={(page) => setQuery((previous) => ({ ...previous, page }))}
        />
      )}
      {editor && allowed && (
        <MediaEditor
          client={client}
          entry={editor === "new" ? undefined : editor}
          onClose={() => setEditor(null)}
          onSaved={saved}
          onAccessFailure={onAccessFailure}
        />
      )}
      {allowed && capabilities && (
        <MediaImport
          open={importing}
          client={client}
          imageLoader={imageLoader}
          capabilities={capabilities}
          onClose={() => setImporting(false)}
          onImported={saved}
          onAccessFailure={onAccessFailure}
        />
      )}
      {gallery && allowed && selected && (
        <MediaGalleryEditor
          client={client}
          entry={selected}
          imageLoader={imageLoader}
          onClose={() => setGallery(false)}
          onUpdated={saved}
          onAccessFailure={onAccessFailure}
        />
      )}
      <AlertDialog open={removing} onOpenChange={setRemoving}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("common.removeConfirm", { title: selected?.title })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t("common.removeHint")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <MediaError error={mutation.error} />
          <AlertDialogFooter>
            <AlertDialogCancel disabled={mutation.pending}>
              {t("common.cancel")}
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={mutation.pending}
              onClick={(event) => {
                event.preventDefault();
                if (selected)
                  void mutation.run(
                    (signal) =>
                      client.remove(selected.id, selected.version, signal),
                    () => {
                      setRemoving(false);
                      setSelected(null);
                      setRevision((value) => value + 1);
                    },
                  );
              }}
            >
              {t("common.remove")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <Dialog
        open={actor !== null}
        onOpenChange={(open) => {
          if (!open) {
            actorTask.cancel();
            setActor(null);
            setCredits(null);
          }
        }}
      >
        <DialogContent className="max-h-[85svh] overflow-y-auto sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>
              {t("entry.actorCredits", { name: actor?.name })}
            </DialogTitle>
            <DialogDescription>{t("entry.source")}</DialogDescription>
          </DialogHeader>
          <MediaError error={actorTask.error} />
          {actorTask.pending ? (
            <MediaLoading />
          ) : credits?.items.length === 0 ? (
            <MediaEmpty title={t("import.none")} />
          ) : (
            <ul className="flex flex-col gap-3">
              {credits?.items.map((candidate) => (
                <li
                  key={`${candidate.identity.provider}:${candidate.identity.mediaType}:${candidate.identity.externalId}`}
                  className="flex gap-3"
                >
                  <MediaImage
                    image={candidate.poster}
                    imageLoader={imageLoader}
                    alt={candidate.title}
                    variant="200"
                    className="aspect-[2/3] w-14 shrink-0 rounded object-cover"
                  />
                  <div>
                    <p className="font-medium">
                      {candidate.title} {candidate.year ?? ""}
                    </p>
                    <p className="line-clamp-3 text-sm text-muted-foreground">
                      {candidate.overview || t("entry.noOverview")}
                    </p>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}
