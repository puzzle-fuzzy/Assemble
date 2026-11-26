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
import { Card, CardContent, CardHeader } from "@assemble/ui/card";
import {
  DataTable,
  type DataTableColumn,
  DataTableColumnFilter,
} from "@assemble/ui/data-table";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@assemble/ui/dialog";
import { Field, FieldGroup, FieldLabel } from "@assemble/ui/field";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@assemble/ui/input-group";

import {
  ArrowLeftIcon,
  CircleCheckIcon,
  CircleDashedIcon,
  ImageIcon,
  LoaderCircleIcon,
  PencilIcon,
  PlusIcon,
  RefreshCwIcon,
  SearchIcon,
  TrashIcon,
  XIcon,
} from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
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
import { MediaEmpty, MediaError, MediaLoading } from "./feedback";
import { MediaGalleryEditor } from "./gallery";
import { MediaImport } from "./import";
import { MediaDetails } from "./media-details";
import { MediaImage } from "./media-image";

const EMPTY_ENTRIES: MediaEntry[] = [];

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
  const searchInput = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState<MediaQuery>({
    page: 1,
    pageSize: 10,
    sort: "updated",
  });
  const [q, setQ] = useState("");
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
  function openEntry(
    id: string,
    action: "view" | "edit" | "gallery" | "remove" = "view",
  ) {
    setSelected(null);
    void detailTask.run(
      (signal) => client.get(id, signal),
      (entry) => {
        setSelected(entry);
        if (!allowed) return;
        if (action === "edit") setEditor(entry);
        if (action === "gallery") setGallery(true);
        if (action === "remove") setRemoving(true);
      },
    );
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
  const columns: DataTableColumn<MediaEntry>[] = [
    {
      id: "title",
      label: t("entry.title"),
      hideable: false,
      className: "min-w-52 max-w-96 whitespace-normal",
      cell: (entry) => (
        <>
          <Button
            variant="link"
            className="h-auto justify-start whitespace-normal p-0 text-left wrap-anywhere"
            aria-label={t("common.details", { title: entry.title })}
            onClick={() => openEntry(entry.id)}
          >
            {entry.title}
          </Button>
          {entry.originalTitle && entry.originalTitle !== entry.title && (
            <p className="text-xs text-muted-foreground">
              {entry.originalTitle}
            </p>
          )}
        </>
      ),
    },
    {
      id: "type",
      label: t("entry.mediaType"),
      header: (
        <DataTableColumnFilter
          label={t("entry.mediaType")}
          value={query.mediaType ?? "all"}
          options={(["all", "movie", "tv"] as const).map((value) => ({
            value,
            label: t(`type.${value}`),
          }))}
          onValueChange={(value) => {
            if (value !== "all" && value !== "movie" && value !== "tv") return;
            setQuery((previous) => ({
              ...previous,
              mediaType: value === "all" ? undefined : value,
              page: 1,
            }));
          }}
        />
      ),
      cell: (entry) => (
        <Badge variant="outline" className="px-1.5 text-muted-foreground">
          {t(`type.${entry.mediaType}`)}
        </Badge>
      ),
    },
    {
      id: "year",
      label: t("entry.year"),
      cell: (entry) => entry.year ?? t("common.unknown"),
    },
    {
      id: "status",
      label: t("entry.status"),
      header: (
        <DataTableColumnFilter
          label={t("entry.status")}
          value={query.status ?? "all"}
          options={(["all", "planned", "watching", "completed"] as const).map(
            (value) => ({ value, label: t(`status.${value}`) }),
          )}
          onValueChange={(value) => {
            if (
              value !== "all" &&
              value !== "planned" &&
              value !== "watching" &&
              value !== "completed"
            )
              return;
            setQuery((previous) => ({
              ...previous,
              status: value === "all" ? undefined : value,
              page: 1,
            }));
          }}
        />
      ),
      cell: (entry) => (
        <Badge variant="outline" className="px-1.5 text-muted-foreground">
          {entry.status === "completed" ? (
            <CircleCheckIcon
              className="fill-success text-background"
              aria-hidden="true"
            />
          ) : entry.status === "watching" ? (
            <LoaderCircleIcon aria-hidden="true" />
          ) : (
            <CircleDashedIcon aria-hidden="true" />
          )}
          {t(`status.${entry.status}`)}
        </Badge>
      ),
    },
    {
      id: "watchedAt",
      label: t("entry.watchedAt"),
      cell: (entry) => entry.watchedAt ?? t("common.unknown"),
    },
  ];
  return (
    <div className="@container/media flex min-w-0 flex-col gap-4">
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
              <div className="flex max-w-full flex-wrap gap-2">
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
                  </>
                )}
              </div>
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
      ) : (
        <DataTable
          label={t("list.showing")}
          data={
            listTask.pending || detailTask.pending
              ? EMPTY_ENTRIES
              : (list?.items ?? EMPTY_ENTRIES)
          }
          columns={columns}
          getRowLabel={(entry) => entry.title}
          pagination={{
            page: list?.page ?? query.page ?? 1,
            pageSize: list?.pageSize ?? query.pageSize ?? 10,
            total: list?.total ?? 0,
            pending: listTask.pending || detailTask.pending,
            onChange: (page, pageSize) =>
              setQuery((previous) => ({ ...previous, page, pageSize })),
          }}
          toolbar={
            <search aria-label={t("list.query")} className="w-full max-w-sm">
              <form
                aria-label={t("list.query")}
                onSubmit={(event) => {
                  event.preventDefault();
                  setSelected(null);
                  setQuery((previous) => ({
                    ...previous,
                    q: q.trim() || undefined,
                    page: 1,
                  }));
                }}
              >
                <FieldGroup>
                  <Field>
                    <FieldLabel htmlFor={`${prefix}-q`} className="sr-only">
                      {t("list.query")}
                    </FieldLabel>
                    <div className="flex items-center gap-2">
                      <InputGroup>
                        <InputGroupInput
                          ref={searchInput}
                          id={`${prefix}-q`}
                          type="search"
                          name="q"
                          placeholder={t("list.placeholder")}
                          value={q}
                          maxLength={300}
                          autoCorrect="off"
                          autoCapitalize="none"
                          spellCheck={false}
                          enterKeyHint="search"
                          className="[&::-webkit-search-cancel-button]:appearance-none"
                          onChange={(event) => setQ(event.target.value)}
                        />
                        {(q || query.q || query.mediaType || query.status) && (
                          <InputGroupAddon align="inline-end">
                            <InputGroupButton
                              size="icon-xs"
                              aria-label={t("list.reset")}
                              title={t("list.reset")}
                              onClick={() => {
                                setQ("");
                                setSelected(null);
                                setQuery((previous) => ({
                                  page: 1,
                                  pageSize: previous.pageSize ?? 10,
                                  sort: "updated",
                                }));
                                searchInput.current?.focus();
                              }}
                            >
                              <XIcon aria-hidden="true" />
                            </InputGroupButton>
                          </InputGroupAddon>
                        )}
                      </InputGroup>
                      <Button type="submit" size="sm">
                        <SearchIcon
                          data-icon="inline-start"
                          aria-hidden="true"
                        />
                        {t("common.search")}
                      </Button>
                    </div>
                  </Field>
                </FieldGroup>
              </form>
            </search>
          }
          actions={
            <>
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
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setImporting(true)}
                  >
                    <SearchIcon data-icon="inline-start" />
                    {t("common.import")}
                  </Button>
                </>
              )}
            </>
          }
          rowActions={(entry) => [
            {
              label: t("common.viewDetails"),
              onSelect: () => openEntry(entry.id),
            },
            ...(allowed
              ? [
                  {
                    label: t("common.edit"),
                    onSelect: () => openEntry(entry.id, "edit"),
                  },
                  {
                    label: t("entry.gallery"),
                    onSelect: () => openEntry(entry.id, "gallery"),
                  },
                  {
                    label: t("common.remove"),
                    destructive: true,
                    onSelect: () => openEntry(entry.id, "remove"),
                  },
                ]
              : []),
          ]}
          emptyState={
            listTask.pending || detailTask.pending ? (
              <MediaLoading />
            ) : listTask.error ? (
              <MediaError
                error={listTask.error}
                retry={() => setRevision((value) => value + 1)}
              />
            ) : (
              <p className="text-sm text-muted-foreground">
                {t(
                  query.q || query.mediaType || query.status
                    ? "common.noMatches"
                    : "common.empty",
                )}
              </p>
            )
          }
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
