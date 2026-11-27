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
  usePageCorrection,
  useUrlState,
} from "@assemble/ui/hooks/use-url-state";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@assemble/ui/input-group";
import { PageBreadcrumbs } from "@assemble/ui/page-breadcrumbs";

import {
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
import {
  type ReactNode,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import type { MediaClient } from "../client";
import { mediaQueryCodec } from "../hooks/list-location";
import { type MediaAccessFailure, useMediaTask } from "../hooks/use-media-task";
import { useMediaTranslation } from "../i18n";
import type {
  ImageLoader,
  MediaActorCredits,
  MediaCapabilities,
  MediaEntry,
  MediaList,
  MediaPersonIdentity,
  MediaType,
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
import { MediaRecordedTime } from "./recorded-time";

const EMPTY_ENTRIES: MediaEntry[] = [];

export interface MediaPageLocation {
  accountKey: string;
  id: string;
  title: string;
  showList: () => void;
}

export interface MediaWorkspaceProps {
  listLocation?: string;
  presentation?: "default" | "management";
  toolbarActions?: ReactNode;
  client: MediaClient;
  imageLoader: ImageLoader;
  canEdit: boolean;
  accountKey: string;
  onAccessFailure?: (failure: MediaAccessFailure) => void;
  onCopyMagnet?: (entry: MediaEntry) => Promise<void>;
  onSignIn?: () => void;
  onLocationChange?: (location: MediaPageLocation | null) => void;
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
  listLocation,
  presentation = "default",
  toolbarActions,
  client,
  imageLoader,
  canEdit,
  accountKey,
  onAccessFailure,
  onCopyMagnet,
  onSignIn,
  onLocationChange,
}: MediaWorkspaceProps) {
  const { t } = useMediaTranslation();
  const prefix = useId();
  const searchInput = useRef<HTMLInputElement>(null);
  const defaultPageSize = presentation === "management" ? 20 : 10;
  const codec = useMemo(
    () => mediaQueryCodec(defaultPageSize),
    [defaultPageSize],
  );
  const [query, setQuery] = useUrlState(codec, listLocation);
  const [q, setQ] = useState(query.q ?? "");
  useEffect(() => setQ(query.q ?? ""), [query.q]);
  const [list, setList] = useState<MediaList | null>(null);
  const correctPage = useCallback(
    (page: number, pageSize: number) =>
      setQuery((previous) => ({ ...previous, page, pageSize }), true),
    [setQuery],
  );
  usePageCorrection(list, correctPage);
  const [capabilities, setCapabilities] = useState<MediaCapabilities | null>(
    null,
  );
  const [selected, setSelected] = useState<MediaEntry | null>(null);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const detailTrigger = useRef<HTMLElement | null>(null);
  const [editor, setEditor] = useState<MediaEntry | "new" | null>(null);
  const [importing, setImporting] = useState(false);
  const [gallery, setGallery] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [actor, setActor] = useState<{
    identity: MediaPersonIdentity;
    name: string;
    mediaType: MediaType;
  } | null>(null);
  const [credits, setCredits] = useState<MediaActorCredits | null>(null);
  const [revision, setRevision] = useState(0);
  const listTask = useMediaTask(onAccessFailure);
  const detailTask = useMediaTask(onAccessFailure);
  const actorTask = useMediaTask(onAccessFailure);
  const mutation = useMediaTask(onAccessFailure);
  const showList = useCallback(() => {
    detailTask.cancel();
    setDetailsOpen(false);
    setSelected(null);
  }, [detailTask.cancel]);
  useEffect(() => {
    if (presentation === "management" || !selected) {
      onLocationChange?.(null);
      return;
    }
    onLocationChange?.({
      accountKey,
      id: selected.id,
      title: selected.title,
      showList,
    });
    return () => onLocationChange?.(null);
  }, [accountKey, presentation, selected, showList, onLocationChange]);
  useEffect(() => {
    detailTask.cancel();
    setSelected(null);
    setDetailsOpen(false);
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
    if (presentation === "management") setDetailsOpen(false);
    setSelected(entry);
    setEditor(null);
    setImporting(false);
    setRevision((value) => value + 1);
  }
  function openEntry(
    id: string,
    action: "view" | "edit" | "gallery" | "remove" = "view",
  ) {
    detailTrigger.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    setDetailsOpen(action === "view");
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
  function showActor(
    identity: MediaPersonIdentity,
    name: string,
    page = 1,
    mediaType = selected?.mediaType ?? "movie",
  ) {
    setActor({ identity, name, mediaType });
    setCredits(null);
    void actorTask.run(
      (signal) =>
        client.actor(
          identity,
          mediaType,
          { page, pageSize: presentation === "management" ? 20 : 24 },
          signal,
        ),
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
            className="h-auto min-h-8 justify-start whitespace-normal p-0 text-left wrap-anywhere"
            aria-label={t("common.details", { title: entry.title })}
            onClick={() => openEntry(entry.id)}
          >
            {entry.title}
          </Button>
          {presentation !== "management" &&
            entry.originalTitle &&
            entry.originalTitle !== entry.title && (
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
      cell: (entry) =>
        presentation === "management" ? (
          t(`type.${entry.mediaType}`)
        ) : (
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
      cell: (entry) =>
        presentation === "management" ? (
          t(`status.${entry.status}`)
        ) : (
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
    ...(presentation === "management"
      ? [
          {
            id: "owner",
            label: t("publication.owner"),
            header: (
              <DataTableColumnFilter
                label={t("publication.owner")}
                value={query.scope ?? "all"}
                options={[
                  { value: "all", label: t("publication.allLibraries") },
                  { value: "shared", label: t("scope.shared") },
                  { value: "private", label: t("publication.userLibraries") },
                ]}
                onValueChange={(value) => {
                  if (
                    value === "all" ||
                    value === "shared" ||
                    value === "private"
                  )
                    setQuery((previous) => ({
                      ...previous,
                      page: 1,
                      scope: value === "all" ? undefined : value,
                    }));
                }}
              />
            ),
            cell: (entry: MediaEntry) => (
              <div className="flex flex-col gap-1">
                <span>{entry.owner?.displayName ?? t("scope.shared")}</span>
                {entry.owner && (
                  <span className="break-all text-xs text-muted-foreground">
                    {entry.owner.id}
                  </span>
                )}
              </div>
            ),
          },
          {
            id: "publication",
            label: t("publication.visibility"),
            cell: (entry: MediaEntry) => (
              <Badge variant="outline">
                {t(
                  entry.scope === "shared" || entry.published
                    ? "publication.public"
                    : "publication.private",
                )}
              </Badge>
            ),
          },
          {
            id: "updatedAt",
            label: t("common.updatedAt"),
            cell: (entry: MediaEntry) => (
              <MediaRecordedTime value={entry.updatedAt} />
            ),
          },
        ]
      : []),
  ];
  return (
    <div className="@container/media flex min-w-0 flex-col gap-4">
      <MediaError error={detailTask.error} />
      {selected && presentation !== "management" ? (
        <Card>
          <CardHeader>
            <div className="flex flex-wrap justify-between gap-2">
              {!onLocationChange && (
                <PageBreadcrumbs
                  ancestors={[
                    {
                      id: "library",
                      label: t(`scope.${client.scope}`),
                      href: "#media",
                      onNavigate: showList,
                    },
                  ]}
                  current={selected.title}
                />
              )}
              <div className="ml-auto flex max-w-full flex-wrap gap-2">
                <Button
                  variant="outline"
                  size="default"
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
                      size="default"
                      onClick={() => setEditor(selected)}
                    >
                      <PencilIcon data-icon="inline-start" />
                      {t("common.edit")}
                    </Button>
                    <Button
                      variant="outline"
                      size="default"
                      onClick={() => setGallery(true)}
                    >
                      <ImageIcon data-icon="inline-start" />
                      {t("entry.gallery")}
                    </Button>
                    <Button
                      variant="destructive"
                      size="default"
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
          variant={presentation}
          label={t("list.showing")}
          data={
            listTask.pending ||
            (detailTask.pending && presentation !== "management")
              ? EMPTY_ENTRIES
              : (list?.items ?? EMPTY_ENTRIES)
          }
          columns={columns}
          getRowLabel={(entry) => entry.title}
          pagination={{
            page: list?.page ?? query.page ?? 1,
            pageSize: list?.pageSize ?? query.pageSize ?? defaultPageSize,
            total: list?.total ?? 0,
            pending: listTask.pending || detailTask.pending,
            onChange: (page, pageSize) =>
              setQuery((previous) => ({
                ...previous,
                page: pageSize === previous.pageSize ? page : 1,
                pageSize,
              })),
          }}
          toolbar={
            <search
              aria-label={t("list.query")}
              className={
                presentation === "management"
                  ? "w-full max-w-80"
                  : "w-full max-w-sm"
              }
            >
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
                          placeholder={t(
                            presentation === "management"
                              ? "publication.search"
                              : "list.placeholder",
                          )}
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
                          <InputGroupAddon align="inline-end" className="py-0">
                            <InputGroupButton
                              size="icon"
                              aria-label={t("list.reset")}
                              title={t("list.reset")}
                              onClick={() => {
                                setQ("");
                                setSelected(null);
                                setQuery((previous) => ({
                                  page: 1,
                                  pageSize:
                                    previous.pageSize ?? defaultPageSize,
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
                      <Button
                        type="submit"
                        size="default"
                        variant={
                          presentation === "management" ? "outline" : "default"
                        }
                      >
                        {presentation !== "management" && (
                          <SearchIcon
                            data-icon="inline-start"
                            aria-hidden="true"
                          />
                        )}
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
              {toolbarActions}
              <Button
                variant="outline"
                size="default"
                disabled={listTask.pending}
                onClick={() => {
                  detailTask.cancel();
                  setSelected(null);
                  setRevision((value) => value + 1);
                }}
              >
                {presentation !== "management" && (
                  <RefreshCwIcon data-icon="inline-start" />
                )}
                {t("common.refresh")}
              </Button>
              {allowed && (
                <>
                  <Button
                    variant={
                      presentation === "management" ? "default" : "outline"
                    }
                    size="default"
                    onClick={() => setEditor("new")}
                  >
                    <PlusIcon data-icon="inline-start" />
                    {t("common.create")}
                  </Button>
                  <Button
                    variant="outline"
                    size="default"
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
      {presentation === "management" && (
        <Dialog
          open={detailsOpen}
          onOpenChange={(open) => {
            setDetailsOpen(open);
            if (!open) {
              detailTask.cancel();
              setSelected(null);
            }
          }}
        >
          <DialogContent
            className="media-management-dialog max-h-[90svh] overflow-y-auto sm:max-w-6xl"
            aria-describedby={undefined}
            onCloseAutoFocus={(event) => {
              if (detailTrigger.current?.isConnected) {
                event.preventDefault();
                detailTrigger.current.focus();
              }
            }}
          >
            <DialogHeader className="pr-8">
              <DialogTitle>
                {selected?.title ?? t("common.viewDetails")}
              </DialogTitle>
            </DialogHeader>
            {detailTask.pending ? (
              <MediaLoading />
            ) : detailTask.error ? (
              <MediaError error={detailTask.error} />
            ) : (
              selected && (
                <>
                  {allowed && (
                    <div className="flex flex-wrap gap-2">
                      <Button
                        variant="outline"
                        size="default"
                        onClick={() => setEditor(selected)}
                      >
                        {t("common.edit")}
                      </Button>
                      <Button
                        variant="outline"
                        size="default"
                        onClick={() => setGallery(true)}
                      >
                        {t("entry.gallery")}
                      </Button>
                      <Button
                        variant="destructive"
                        size="default"
                        onClick={() => setRemoving(true)}
                      >
                        {t("common.remove")}
                      </Button>
                    </div>
                  )}
                  <MediaDetails
                    key={`${selected.id}:${selected.version}`}
                    entry={selected}
                    imageLoader={imageLoader}
                    presentation="management"
                    galleryClient={client}
                    onAccessFailure={onAccessFailure}
                    onActor={showActor}
                    onCopyMagnet={onCopyMagnet}
                    onSignIn={onSignIn}
                  />
                </>
              )
            )}
          </DialogContent>
        </Dialog>
      )}
      {editor && allowed && (
        <MediaEditor
          client={client}
          viewerAccountId={accountKey}
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
                    compact
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
          {credits && actor && (
            <MediaPagination
              page={credits.page}
              pageSize={credits.pageSize}
              total={credits.total}
              pending={actorTask.pending}
              onPage={(page) =>
                showActor(actor.identity, actor.name, page, actor.mediaType)
              }
            />
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}
