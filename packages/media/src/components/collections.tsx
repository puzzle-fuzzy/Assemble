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
import { Button } from "@assemble/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@assemble/ui/card";
import { DataTable, type DataTableColumn } from "@assemble/ui/data-table";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@assemble/ui/dialog";
import { Field, FieldGroup, FieldLabel } from "@assemble/ui/field";
import { Input } from "@assemble/ui/input";
import { Textarea } from "@assemble/ui/textarea";
import { PlusIcon, RefreshCwIcon } from "lucide-react";
import {
  type ReactNode,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import type { MediaClient, MediaCollectionsClient } from "../client";
import {
  type MediaAccessFailure,
  mediaErrorCode,
  useMediaTask,
} from "../hooks/use-media-task";
import { useMediaTranslation } from "../i18n";
import type {
  ImageLoader,
  MediaCollection,
  MediaCollectionList,
  MediaEntry,
  MediaList,
} from "../model";
import {
  MediaEmpty,
  MediaError,
  MediaLoading,
  MediaPagination,
} from "./feedback";
import { MediaDetails } from "./media-details";
import { MediaImage } from "./media-image";
import { MediaRecordedTime } from "./recorded-time";

export interface MediaCollectionsManagerProps {
  presentation?: "default" | "management";
  toolbarActions?: ReactNode;
  client: MediaCollectionsClient;
  libraryClient: MediaClient;
  imageLoader: ImageLoader;
  accountKey: string;
  canEdit?: boolean;
  onAccessFailure?: (failure: MediaAccessFailure) => void;
  onCopyMagnet?: (entry: MediaEntry) => Promise<void>;
}
export function MediaCollectionsManager(props: MediaCollectionsManagerProps) {
  return <CollectionsContent key={props.accountKey} {...props} />;
}
function CollectionsContent({
  presentation = "default",
  toolbarActions,
  client,
  libraryClient,
  imageLoader,
  canEdit = true,
  onAccessFailure,
  onCopyMagnet,
}: MediaCollectionsManagerProps) {
  const { t } = useMediaTranslation();
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(
    presentation === "management" ? 10 : 12,
  );
  const [revision, setRevision] = useState(0);
  const [list, setList] = useState<MediaCollectionList | null>(null);
  const [editing, setEditing] = useState<MediaCollection | "new" | null>(null);
  const [removing, setRemoving] = useState<MediaCollection | null>(null);
  const [selected, setSelected] = useState<MediaEntry | null>(null);
  const [collectionDetails, setCollectionDetails] =
    useState<MediaCollection | null>(null);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const detailTrigger = useRef<HTMLElement | null>(null);
  const task = useMediaTask(onAccessFailure);
  const mutation = useMediaTask(onAccessFailure);
  const detailTask = useMediaTask(onAccessFailure);
  const collectionTask = useMediaTask(onAccessFailure);
  useEffect(() => {
    void revision;
    setList(null);
    void task.run((signal) => client.list({ page, pageSize }, signal), setList);
  }, [client, page, pageSize, revision, task.run]);
  function showCollection(collection: MediaCollection) {
    detailTrigger.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    setCollectionDetails(null);
    setDetailsOpen(true);
    void collectionTask.run(
      (signal) => client.get(collection.id, signal),
      setCollectionDetails,
    );
  }
  const columns: DataTableColumn<MediaCollection>[] = [
    {
      id: "title",
      label: t("collections.name"),
      hideable: false,
      className: "min-w-40 max-w-72 whitespace-normal break-words",
      cell: (collection) => collection.title,
    },
    {
      id: "description",
      label: t("collections.description"),
      className: "max-w-96 whitespace-normal break-words",
      cell: (collection) => collection.description || t("common.unknown"),
    },
    {
      id: "members",
      label: t("collections.members"),
      cell: (collection) =>
        t("collections.count", { count: collection.items.length }),
    },
    {
      id: "updatedAt",
      label: t("common.updatedAt"),
      cell: (collection) => <MediaRecordedTime value={collection.updatedAt} />,
    },
  ];
  return (
    <div className="flex flex-col gap-4">
      {presentation === "management" ? (
        <DataTable
          variant="management"
          label={t("collections.title")}
          data={list?.items ?? []}
          columns={columns}
          getRowLabel={(collection) => collection.title}
          pagination={{
            page: list?.page ?? page,
            pageSize: list?.pageSize ?? pageSize,
            total: list?.total ?? 0,
            pending: task.pending,
            onChange: (nextPage, nextSize) => {
              setPage(nextPage);
              setPageSize(nextSize);
            },
          }}
          actions={
            <>
              {toolbarActions}
              <Button
                variant="outline"
                size="default"
                disabled={task.pending}
                onClick={() => setRevision((value) => value + 1)}
              >
                {t("common.refresh")}
              </Button>
              {canEdit && (
                <Button size="default" onClick={() => setEditing("new")}>
                  {t("collections.create")}
                </Button>
              )}
            </>
          }
          rowActions={(collection) => [
            {
              label: t("common.viewDetails"),
              onSelect: () => showCollection(collection),
            },
            ...(canEdit
              ? [
                  {
                    label: t("common.edit"),
                    onSelect: () =>
                      void detailTask.run(
                        (signal) => client.get(collection.id, signal),
                        setEditing,
                      ),
                  },
                  {
                    label: t("common.remove"),
                    destructive: true,
                    onSelect: () => setRemoving(collection),
                  },
                ]
              : []),
          ]}
          emptyState={
            task.pending ? (
              <MediaLoading />
            ) : task.error ? (
              <MediaError
                error={task.error}
                retry={() => setRevision((value) => value + 1)}
              />
            ) : (
              <MediaEmpty title={t("collections.empty")} />
            )
          }
        />
      ) : (
        <>
          <Card>
            <CardHeader>
              <CardTitle>{t("collections.title")}</CardTitle>
              <CardDescription>{t("collections.hint")}</CardDescription>
            </CardHeader>
            <CardContent className="flex flex-wrap gap-2">
              {toolbarActions}
              <Button
                variant="outline"
                disabled={task.pending}
                onClick={() => setRevision((value) => value + 1)}
              >
                <RefreshCwIcon data-icon="inline-start" />
                {t("common.refresh")}
              </Button>
              {canEdit && (
                <Button onClick={() => setEditing("new")}>
                  <PlusIcon data-icon="inline-start" />
                  {t("collections.create")}
                </Button>
              )}
            </CardContent>
          </Card>
          <MediaError
            error={task.error}
            retry={() => setRevision((value) => value + 1)}
          />
          <MediaError error={detailTask.error} />
          {task.pending ? (
            <MediaLoading />
          ) : list?.items.length === 0 ? (
            <MediaEmpty title={t("collections.empty")} />
          ) : (
            <div className="grid items-start gap-4 lg:grid-cols-2">
              {list?.items.map((collection) => (
                <Card key={collection.id}>
                  <CardHeader>
                    <CardTitle>{collection.title}</CardTitle>
                    <CardDescription>
                      {collection.description ||
                        t("collections.count", {
                          count: collection.items.length,
                        })}
                    </CardDescription>
                  </CardHeader>
                  <CardContent className="flex flex-col gap-4">
                    <ul className="flex flex-col gap-2">
                      {collection.items.map((entry) => (
                        <li key={entry.id}>
                          <Button
                            variant="link"
                            className="h-auto min-h-8 justify-start whitespace-normal p-0 text-left"
                            onClick={() =>
                              void detailTask.run(
                                (signal) => libraryClient.get(entry.id, signal),
                                setSelected,
                              )
                            }
                          >
                            {entry.title}
                          </Button>
                        </li>
                      ))}
                    </ul>
                    {canEdit && (
                      <div className="flex gap-2">
                        <Button
                          variant="outline"
                          size="default"
                          onClick={() =>
                            void detailTask.run(
                              (signal) => client.get(collection.id, signal),
                              setEditing,
                            )
                          }
                        >
                          {t("collections.edit")}
                        </Button>
                        <Button
                          variant="destructive"
                          size="default"
                          onClick={() => setRemoving(collection)}
                        >
                          {t("common.remove")}
                        </Button>
                      </div>
                    )}
                  </CardContent>
                </Card>
              ))}
            </div>
          )}
          {list && (
            <MediaPagination
              page={list.page}
              total={list.total}
              pageSize={list.pageSize}
              pending={task.pending}
              onPage={setPage}
            />
          )}
        </>
      )}
      {presentation === "management" && (
        <>
          <MediaError error={detailTask.error} />
          <Dialog open={detailsOpen} onOpenChange={setDetailsOpen}>
            <DialogContent
              className="max-h-[90svh] overflow-y-auto sm:max-w-3xl"
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
                  {collectionDetails?.title ?? t("common.viewDetails")}
                </DialogTitle>
              </DialogHeader>
              {collectionTask.pending ? (
                <MediaLoading />
              ) : collectionTask.error ? (
                <MediaError error={collectionTask.error} />
              ) : (
                collectionDetails && (
                  <>
                    {collectionDetails.description && (
                      <p className="break-words text-sm">
                        {collectionDetails.description}
                      </p>
                    )}
                    <ol className="flex flex-col gap-2">
                      {collectionDetails.items.map((entry) => (
                        <li key={entry.id}>
                          <Button
                            variant="link"
                            className="h-auto min-h-8 justify-start p-0 whitespace-normal text-left"
                            onClick={() =>
                              void detailTask.run(
                                (signal) => libraryClient.get(entry.id, signal),
                                setSelected,
                              )
                            }
                          >
                            {entry.title}
                          </Button>
                        </li>
                      ))}
                    </ol>
                    {collectionDetails.items.length === 0 && (
                      <MediaEmpty title={t("collections.noMembers")} />
                    )}
                  </>
                )
              )}
            </DialogContent>
          </Dialog>
        </>
      )}
      {editing && canEdit && (
        <CollectionEditor
          client={client}
          libraryClient={libraryClient}
          imageLoader={imageLoader}
          entry={editing === "new" ? undefined : editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            setRevision((value) => value + 1);
          }}
          onAccessFailure={onAccessFailure}
        />
      )}
      <AlertDialog
        open={removing !== null}
        onOpenChange={(open) => {
          if (!open && !mutation.pending) setRemoving(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("collections.removeConfirm", { title: removing?.title })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t("collections.removeHint")}
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
                if (removing)
                  void mutation.run(
                    (signal) =>
                      client.remove(removing.id, removing.version, signal),
                    () => {
                      setRemoving(null);
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
        open={selected !== null}
        onOpenChange={(open) => {
          if (!open) setSelected(null);
        }}
      >
        <DialogContent
          className={
            presentation === "management"
              ? "media-management-dialog max-h-[90svh] overflow-y-auto sm:max-w-6xl"
              : "max-h-[90svh] overflow-y-auto sm:max-w-3xl"
          }
        >
          <DialogHeader>
            <DialogTitle>{selected?.title}</DialogTitle>
            <DialogDescription>{t("scope.shared")}</DialogDescription>
          </DialogHeader>
          {selected && (
            <MediaDetails
              entry={selected}
              presentation={
                presentation === "management" ? "management" : "workspace"
              }
              galleryClient={libraryClient}
              onAccessFailure={onAccessFailure}
              imageLoader={imageLoader}
              onCopyMagnet={onCopyMagnet}
            />
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}
function CollectionEditor({
  client,
  libraryClient,
  imageLoader,
  entry,
  onClose,
  onSaved,
  onAccessFailure,
}: {
  client: MediaCollectionsClient;
  libraryClient: MediaClient;
  imageLoader: ImageLoader;
  entry?: MediaCollection;
  onClose: () => void;
  onSaved: () => void;
  onAccessFailure?: (failure: MediaAccessFailure) => void;
}) {
  const { t } = useMediaTranslation();
  const prefix = useId();
  const [base, setBase] = useState(entry);
  const [title, setTitle] = useState(entry?.title ?? "");
  const [description, setDescription] = useState(entry?.description ?? "");
  const [members, setMembers] = useState(entry?.items ?? []);
  const membersSection = useRef<HTMLElement>(null);
  const movedMember = useRef<string | null>(null);
  useLayoutEffect(() => {
    const id = movedMember.current;
    movedMember.current = null;
    if (id && members.some((member) => member.id === id))
      [
        ...(membersSection.current?.querySelectorAll<HTMLTableRowElement>(
          "tr[data-table-row]",
        ) ?? []),
      ]
        .find((row) => row.dataset.tableRow === id)
        ?.querySelector<HTMLButtonElement>("[data-table-drag-handle]")
        ?.focus({ preventScroll: true });
  }, [members]);
  const [q, setQ] = useState("");
  const [query, setQuery] = useState<{ q: string; page: number } | null>(null);
  const [results, setResults] = useState<MediaList | null>(null);
  const search = useMediaTask(onAccessFailure);
  const task = useMediaTask(onAccessFailure);
  const requestId = useRef(crypto.randomUUID());
  const conflict = mediaErrorCode(task.error) === "VERSION_CONFLICT";
  useEffect(() => {
    if (query) {
      setResults(null);
      void search.run(
        (signal) => libraryClient.list({ ...query, pageSize: 10 }, signal),
        setResults,
      );
    }
  }, [libraryClient, query, search.run]);
  function move(index: number, delta: number) {
    movedMember.current = members[index]?.id ?? null;
    setMembers((previous) => {
      const next = [...previous];
      [next[index], next[index + delta]] = [next[index + delta], next[index]];
      return next;
    });
  }
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !task.pending) onClose();
      }}
    >
      <DialogContent className="max-h-[90svh] overflow-y-auto sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>
            {t(base ? "collections.edit" : "collections.create")}
          </DialogTitle>
          <DialogDescription>{t("collections.hint")}</DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            const content = {
              title: title.trim(),
              description: description.trim(),
              entryIds: members.map((item) => item.id),
            };
            void task.run(
              (signal) =>
                base
                  ? client.update(base.id, base.version, content, signal)
                  : client.create(content, requestId.current, signal),
              onSaved,
            );
          }}
        >
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor={`${prefix}-title`}>
                {t("collections.name")}
              </FieldLabel>
              <Input
                id={`${prefix}-title`}
                value={title}
                required
                maxLength={1000}
                disabled={task.pending}
                onChange={(event) => setTitle(event.target.value)}
              />
            </Field>
            <Field>
              <FieldLabel htmlFor={`${prefix}-description`}>
                {t("collections.description")}
              </FieldLabel>
              <Textarea
                id={`${prefix}-description`}
                value={description}
                maxLength={20000}
                disabled={task.pending}
                onChange={(event) => setDescription(event.target.value)}
              />
            </Field>
          </FieldGroup>
          <section ref={membersSection} className="flex flex-col gap-3">
            <h3 className="font-medium">{t("collections.members")}</h3>
            {members.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                {t("collections.noMembers")}
              </p>
            ) : (
              <DataTable
                variant="management"
                label={t("collections.members")}
                data={members}
                getRowLabel={(member) => member.title}
                selectable={!task.pending}
                reorderable={!task.pending}
                onReorder={setMembers}
                paginate={false}
                columns={[
                  {
                    id: "title",
                    label: t("collections.members"),
                    hideable: false,
                    className:
                      "min-w-40 max-w-96 whitespace-normal break-words",
                    cell: (member) =>
                      `${members.findIndex((item) => item.id === member.id) + 1}. ${member.title}`,
                  },
                ]}
                rowActions={(member) => {
                  const index = members.findIndex(
                    (item) => item.id === member.id,
                  );
                  return [
                    {
                      id: "up",
                      label: t("collections.up"),
                      disabled: index === 0 || task.pending,
                      onSelect: () => move(index, -1),
                    },
                    {
                      id: "down",
                      label: t("collections.down"),
                      disabled: index === members.length - 1 || task.pending,
                      onSelect: () => move(index, 1),
                    },
                    {
                      id: "remove",
                      label: t("collections.removeMember"),
                      disabled: task.pending,
                      onSelect: () =>
                        setMembers((previous) =>
                          previous.filter((item) => item.id !== member.id),
                        ),
                    },
                  ];
                }}
              />
            )}
          </section>
          <MediaError error={task.error} />
          {conflict && base && (
            <Button
              variant="outline"
              type="button"
              disabled={task.pending}
              onClick={() =>
                void task.run(
                  (signal) => client.get(base.id, signal),
                  (latest) => {
                    setBase(latest);
                    setTitle(latest.title);
                    setDescription(latest.description);
                    setMembers(latest.items);
                  },
                )
              }
            >
              {t("entry.reload")}
            </Button>
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
            <Button
              type="submit"
              disabled={task.pending || conflict || !title.trim()}
            >
              {t("common.save")}
            </Button>
          </DialogFooter>
        </form>
        <form
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            setQuery({ q: q.trim(), page: 1 });
          }}
        >
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor={`${prefix}-search`}>
                {t("collections.search")}
              </FieldLabel>
              <Input
                id={`${prefix}-search`}
                type="search"
                value={q}
                maxLength={300}
                onChange={(event) => setQ(event.target.value)}
              />
            </Field>
          </FieldGroup>
          <Button
            variant="outline"
            type="submit"
            className="w-fit"
            disabled={task.pending}
          >
            {t("common.search")}
          </Button>
        </form>
        <MediaError error={search.error} />
        {search.pending ? (
          <MediaLoading />
        ) : results?.items.length === 0 ? (
          <MediaEmpty title={t("common.noMatches")} />
        ) : (
          <ul className="flex flex-col gap-2">
            {results?.items.map((item) => (
              <li key={item.id} className="flex items-center gap-3">
                <MediaImage
                  image={item.poster}
                  imageLoader={imageLoader}
                  alt={item.title}
                  variant="200"
                  compact
                  className="aspect-[2/3] w-10 shrink-0 rounded object-cover"
                />
                <p className="min-w-0 flex-1 text-sm">
                  {item.title} {item.year ?? ""}
                </p>
                <Button
                  variant="outline"
                  size="default"
                  disabled={
                    task.pending ||
                    members.some((member) => member.id === item.id)
                  }
                  onClick={() => setMembers((previous) => [...previous, item])}
                >
                  {t(
                    members.some((member) => member.id === item.id)
                      ? "collections.duplicate"
                      : "collections.add",
                  )}
                </Button>
              </li>
            ))}
          </ul>
        )}
        {results && query && (
          <MediaPagination
            page={results.page}
            total={results.total}
            pageSize={results.pageSize}
            pending={search.pending}
            onPage={(page) =>
              setQuery((previous) => (previous ? { ...previous, page } : null))
            }
          />
        )}
      </DialogContent>
    </Dialog>
  );
}
