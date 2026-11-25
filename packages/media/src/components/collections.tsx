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
import {
  ArrowDownIcon,
  ArrowUpIcon,
  PlusIcon,
  RefreshCwIcon,
  XIcon,
} from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
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

export interface MediaCollectionsManagerProps {
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
  client,
  libraryClient,
  imageLoader,
  canEdit = true,
  onAccessFailure,
  onCopyMagnet,
}: MediaCollectionsManagerProps) {
  const { t } = useMediaTranslation();
  const [page, setPage] = useState(1);
  const [revision, setRevision] = useState(0);
  const [list, setList] = useState<MediaCollectionList | null>(null);
  const [editing, setEditing] = useState<MediaCollection | "new" | null>(null);
  const [removing, setRemoving] = useState<MediaCollection | null>(null);
  const [selected, setSelected] = useState<MediaEntry | null>(null);
  const task = useMediaTask(onAccessFailure);
  const mutation = useMediaTask(onAccessFailure);
  const detailTask = useMediaTask(onAccessFailure);
  useEffect(() => {
    void revision;
    setList(null);
    void task.run(
      (signal) => client.list({ page, pageSize: 12 }, signal),
      setList,
    );
  }, [client, page, revision, task.run]);
  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle>{t("collections.title")}</CardTitle>
          <CardDescription>{t("collections.hint")}</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-wrap gap-2">
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
                    t("collections.count", { count: collection.items.length })}
                </CardDescription>
              </CardHeader>
              <CardContent className="flex flex-col gap-4">
                <ul className="flex flex-col gap-2">
                  {collection.items.map((entry) => (
                    <li key={entry.id}>
                      <Button
                        variant="link"
                        className="h-auto justify-start whitespace-normal p-0 text-left"
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
                      size="sm"
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
                      size="sm"
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
        <DialogContent className="max-h-[90svh] overflow-y-auto sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle>{selected?.title}</DialogTitle>
            <DialogDescription>{t("scope.shared")}</DialogDescription>
          </DialogHeader>
          {selected && (
            <MediaDetails
              entry={selected}
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
          <section className="flex flex-col gap-3">
            <h3 className="font-medium">{t("collections.members")}</h3>
            {members.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                {t("collections.noMembers")}
              </p>
            ) : (
              <ol className="flex flex-col gap-2">
                {members.map((member, index) => (
                  <li key={member.id} className="flex items-center gap-2">
                    <span className="min-w-0 flex-1 truncate text-sm">
                      {index + 1}. {member.title}
                    </span>
                    <Button
                      variant="outline"
                      size="icon-sm"
                      type="button"
                      aria-label={`${t("collections.up")} ${member.title}`}
                      disabled={index === 0 || task.pending}
                      onClick={() => move(index, -1)}
                    >
                      <ArrowUpIcon />
                    </Button>
                    <Button
                      variant="outline"
                      size="icon-sm"
                      type="button"
                      aria-label={`${t("collections.down")} ${member.title}`}
                      disabled={index === members.length - 1 || task.pending}
                      onClick={() => move(index, 1)}
                    >
                      <ArrowDownIcon />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      type="button"
                      aria-label={`${t("collections.removeMember")} ${member.title}`}
                      disabled={task.pending}
                      onClick={() =>
                        setMembers((previous) =>
                          previous.filter((item) => item.id !== member.id),
                        )
                      }
                    >
                      <XIcon />
                    </Button>
                  </li>
                ))}
              </ol>
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
                  className="aspect-[2/3] w-10 shrink-0 rounded object-cover"
                />
                <p className="min-w-0 flex-1 text-sm">
                  {item.title} {item.year ?? ""}
                </p>
                <Button
                  variant="outline"
                  size="sm"
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
