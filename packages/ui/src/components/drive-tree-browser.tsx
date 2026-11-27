import { FolderIcon, XIcon } from "lucide-react";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
} from "react";
import type {
  DriveEntry,
  DriveFolder,
  DriveTreeListing,
  DriveTreeQuery,
  DriveTreeSource,
} from "../lib/drive-tree";
import {
  driveTreeError,
  useDriveTreeTranslation,
} from "../lib/drive-tree-i18n";
import { Button } from "./button";
import { DataTable, type DataTableAction } from "./data-table";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./dialog";
import { Field, FieldError, FieldGroup, FieldLabel } from "./field";
import { Input } from "./input";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "./input-group";
import { LoadingState } from "./loading-state";
import {
  type PageBreadcrumbAncestor,
  PageBreadcrumbs,
} from "./page-breadcrumbs";

export interface DriveTreeLocation {
  currentId: string;
  current: string;
  ancestors: PageBreadcrumbAncestor[];
}
interface Props {
  initialParentId?: string;
  onTrash?: (entry: DriveEntry, done: () => void) => void;
  removeLabel?: string;
  renderCapacity?: (listing: DriveTreeListing) => ReactNode;
  variant?: "default" | "management";
  source: DriveTreeSource;
  defaultPageSize?: number;
  refreshKey?: unknown;
  onLocationChange?: (location: DriveTreeLocation | null) => void;
  fileActions?: (
    file: Extract<DriveEntry, { kind: "file" }>,
  ) => DataTableAction[];
  filePreview?: (file: Extract<DriveEntry, { kind: "file" }>) => ReactNode;
  formatSize: (size: number) => string;
  formatDate: (date: string) => ReactNode;
  toolbar?: ReactNode;
}
type Operation =
  | { mode: "create" }
  | { mode: "rename" | "move" | "delete"; entry: DriveEntry };
const initial = (pageSize: number): DriveTreeQuery => ({
  parentId: "root",
  q: "",
  page: 1,
  pageSize,
});
export function DriveTreeBrowser({
  initialParentId = "root",
  onTrash,
  removeLabel,
  renderCapacity,
  variant,
  source,
  defaultPageSize = 10,
  refreshKey,
  onLocationChange,
  fileActions,
  filePreview,
  formatSize,
  formatDate,
  toolbar,
}: Props) {
  const { t } = useDriveTreeTranslation();
  const [query, setQuery] = useState(() => ({
    ...initial(defaultPageSize),
    parentId: initialParentId,
  }));
  const [data, setData] = useState<DriveTreeListing | null>(null);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  const [revision, setRevision] = useState(0);
  const [operation, setOperation] = useState<Operation | null>(null);
  const [uploading, setUploading] = useState(false);
  const [uploadError, setUploadError] = useState<string | null>(null);
  const table = useRef<HTMLDivElement>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const queries = useRef(new Map<string, DriveTreeQuery>());
  const retry = useCallback(() => setRevision((value) => value + 1), []);
  const navigate = useCallback(
    (id: string) => {
      setQuery((previous) => {
        queries.current.set(previous.parentId, previous);
        return (
          queries.current.get(id) ?? {
            ...initial(defaultPageSize),
            parentId: id,
          }
        );
      });
      setData(null);
    },
    [defaultPageSize],
  );
  useEffect(() => {
    void revision;
    void refreshKey;
    const cancel = new AbortController();
    setLoading(true);
    setFailed(false);
    void source.list(query, cancel.signal, true).then(
      (result) => {
        if (!cancel.signal.aborted) {
          setData(result);
          setLoading(false);
        }
      },
      () => {
        if (!cancel.signal.aborted) {
          setFailed(true);
          setLoading(false);
        }
      },
    );
    return () => cancel.abort();
  }, [source, query, revision, refreshKey]);
  useEffect(() => {
    if (!data) {
      onLocationChange?.(null);
      return;
    }
    const folders = [
      ...data.ancestors,
      ...(data.current ? [data.current] : []),
    ];
    onLocationChange?.({
      currentId: data.current?.id ?? "root",
      current: data.current?.name ?? t("root"),
      ancestors: data.current
        ? [
            {
              id: "drive-root",
              label: t("root"),
              href: "#drive",
              onNavigate: () => navigate("root"),
            },
            ...folders.slice(0, -1).map((folder) => ({
              id: folder.id,
              label: folder.name,
              href: `#drive/${folder.id}`,
              onNavigate: () => navigate(folder.id),
            })),
          ]
        : [],
    });
  }, [data, navigate, onLocationChange, t]);
  useEffect(() => () => onLocationChange?.(null), [onLocationChange]);
  const close = () => {
    const id = operation && "entry" in operation ? operation.entry.id : null;
    setOperation(null);
    requestAnimationFrame(() => {
      const target = id
        ? table.current?.querySelector<HTMLButtonElement>(
            `[data-drive-entry-id="${id}"]`,
          )
        : null;
      (target ?? table.current)?.focus();
    });
  };
  return (
    <div
      ref={table}
      tabIndex={-1}
      className="flex min-w-0 flex-col gap-3 outline-none"
    >
      {data ? renderCapacity?.(data) : null}
      <DataTable
        variant={variant}
        label={t("root")}
        data={data?.items ?? []}
        getRowLabel={(entry) => entry.name}
        columns={[
          {
            id: "name",
            label: t("name"),
            hideable: false,
            className: "min-w-56 max-w-96 whitespace-normal break-all",
            cell: (entry) => (
              <div className="flex items-center gap-2">
                {entry.kind === "folder" ? (
                  <Button
                    data-drive-entry-id={entry.id}
                    variant="ghost"
                    className="h-auto min-w-0 whitespace-normal text-start"
                    onClick={() =>
                      entry.kind === "folder" ? navigate(entry.id) : undefined
                    }
                  >
                    {entry.kind === "folder" ? (
                      <FolderIcon data-icon="inline-start" aria-hidden="true" />
                    ) : null}
                    {entry.name}
                  </Button>
                ) : (
                  <span data-drive-entry-id={entry.id} tabIndex={-1}>
                    {entry.name}
                  </span>
                )}
                {entry.kind === "file" ? filePreview?.(entry) : null}
              </div>
            ),
          },
          {
            id: "size",
            label: t("size"),
            cell: (entry) =>
              entry.kind === "folder" ? t("folder") : formatSize(entry.size),
          },
          {
            id: "createdAt",
            label: t("createdAt"),
            cell: (entry) => formatDate(entry.createdAt),
          },
        ]}
        toolbar={
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            {toolbar}
            <search>
              <form onSubmit={(event) => event.preventDefault()}>
                <InputGroup className="w-64 max-w-full">
                  <InputGroupInput
                    ref={searchInput}
                    name="q"
                    aria-label={t("search")}
                    placeholder={t("search")}
                    maxLength={200}
                    value={query.q}
                    onChange={(event) =>
                      setQuery((previous) => ({
                        ...previous,
                        q: event.target.value,
                        page: 1,
                      }))
                    }
                  />
                  {query.q ? (
                    <InputGroupAddon align="inline-end">
                      <InputGroupButton
                        type="button"
                        size="icon-xs"
                        aria-label={t("clear")}
                        onClick={() => {
                          setQuery((previous) => ({
                            ...previous,
                            q: "",
                            page: 1,
                          }));
                          searchInput.current?.focus();
                        }}
                      >
                        <XIcon aria-hidden="true" />
                      </InputGroupButton>
                    </InputGroupAddon>
                  ) : null}
                </InputGroup>
              </form>
            </search>
          </div>
        }
        actions={
          <>
            {source.create ? (
              <Button
                variant="outline"
                disabled={loading}
                onClick={() => setOperation({ mode: "create" })}
              >
                {t("newFolder")}
              </Button>
            ) : null}
            {source.upload ? (
              <Button
                variant="outline"
                disabled={loading || uploading}
                onClick={() => {
                  if (uploading) return;
                  setUploading(true);
                  setUploadError(null);
                  void source
                    .upload?.(query.parentId)
                    .catch((error) => setUploadError(driveTreeError(error)))
                    .finally(() => {
                      setUploading(false);
                      retry();
                    });
                }}
              >
                {t("upload")}
              </Button>
            ) : null}
            <Button variant="outline" disabled={loading} onClick={retry}>
              {t("refresh")}
            </Button>
          </>
        }
        pagination={{
          page: query.page,
          pageSize: query.pageSize,
          total: data?.total ?? 0,
          pending: loading,
          onChange: (page, pageSize) =>
            setQuery((previous) => ({ ...previous, page, pageSize })),
        }}
        rowActions={(entry) => [
          ...(entry.kind === "file" ? (fileActions?.(entry) ?? []) : []),
          ...(source.rename
            ? [
                {
                  id: "rename",
                  label: t("rename"),
                  onSelect: () => setOperation({ mode: "rename", entry }),
                },
              ]
            : []),
          ...(source.move
            ? [
                {
                  id: "move",
                  label: t("move"),
                  onSelect: () => setOperation({ mode: "move", entry }),
                },
              ]
            : []),
          ...(onTrash
            ? [
                {
                  id: "trash",
                  label: removeLabel ?? t("remove"),
                  destructive: true,
                  onSelect: () => onTrash(entry, retry),
                },
              ]
            : []),
          ...(!onTrash && source.remove
            ? [
                {
                  id: "delete",
                  label: t("remove"),
                  destructive: true,
                  onSelect: () => setOperation({ mode: "delete", entry }),
                },
              ]
            : []),
        ]}
        emptyState={
          loading ? (
            <LoadingState label={t("loading")} />
          ) : failed ? (
            <div className="flex flex-col items-center gap-2">
              <p role="alert">{t("loadFailed")}</p>
              <Button variant="outline" onClick={retry}>
                {t("retry")}
              </Button>
            </div>
          ) : (
            <p>{t("empty")}</p>
          )
        }
      />
      {failed && data ? <p role="alert">{t("loadFailed")}</p> : null}
      {uploadError ? <p role="alert">{t(uploadError)}</p> : null}
      {operation ? (
        <OperationDialog
          key={
            operation.mode +
            query.parentId +
            ("entry" in operation ? operation.entry.id : "")
          }
          source={source}
          parentId={query.parentId}
          operation={operation}
          onClose={close}
          onSaved={() => {
            close();
            retry();
          }}
        />
      ) : null}
    </div>
  );
}

function cleanName(value: string): string | null {
  const name = value.trim();
  return name &&
    name.length <= 255 &&
    name !== "." &&
    name !== ".." &&
    !/[/\\\p{Cc}]/u.test(name)
    ? name
    : null;
}
function OperationDialog({
  source,
  parentId,
  operation,
  onClose,
  onSaved,
}: {
  source: DriveTreeSource;
  parentId: string;
  operation: Operation;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useDriveTreeTranslation();
  const id = useId();
  const input = useRef<HTMLInputElement>(null);
  const [name, setName] = useState(
    "entry" in operation ? operation.entry.name : "",
  );
  const [requestId, setRequestId] = useState(() => crypto.randomUUID());
  const [pending, setPending] = useState(false);
  const busy = useRef(false);
  const [error, setError] = useState<string | null>(null);
  const [target, setTarget] = useState(parentId);
  const deleting = operation.mode === "delete";
  const title =
    operation.mode === "create"
      ? t("newFolder")
      : operation.mode === "rename"
        ? t("rename")
        : operation.mode === "move"
          ? t("destination")
          : t(
              operation.entry.kind === "folder" ? "deleteFolder" : "deleteFile",
            );
  async function save() {
    if (busy.current) return;
    const cleaned = cleanName(name);
    if (
      (operation.mode === "create" || operation.mode === "rename") &&
      !cleaned
    ) {
      setError("invalid");
      input.current?.focus();
      return;
    }
    busy.current = true;
    setPending(true);
    setError(null);
    try {
      if (operation.mode === "create")
        await source.create?.({ parentId, name: cleaned ?? name, requestId });
      else if (operation.mode === "rename")
        await source.rename?.(operation.entry.kind, operation.entry.id, {
          name: cleaned ?? name,
          version: operation.entry.version,
        });
      else if (operation.mode === "move")
        await source.move?.({
          kind: operation.entry.kind,
          id: operation.entry.id,
          version: operation.entry.version,
          parentId: target,
        });
      else
        await source.remove?.(
          operation.entry.kind,
          operation.entry.id,
          operation.entry.version,
        );
      onSaved();
    } catch (cause) {
      setError(driveTreeError(cause));
      input.current?.focus();
    } finally {
      busy.current = false;
      setPending(false);
    }
  }
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !pending) onClose();
      }}
    >
      <DialogContent
        aria-describedby={error ? `${id}-error` : undefined}
        className="max-h-[90svh] overflow-y-auto"
        onCloseAutoFocus={(event) => event.preventDefault()}
      >
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
        </DialogHeader>
        <form
          className="flex flex-col gap-4"
          aria-busy={pending}
          noValidate
          onSubmit={(event) => {
            event.preventDefault();
            void save();
          }}
        >
          {operation.mode === "create" || operation.mode === "rename" ? (
            <FieldGroup>
              <Field data-invalid={error === "invalid" || undefined}>
                <FieldLabel className="sr-only" htmlFor={`${id}-name`}>
                  {t(operation.mode === "create" ? "folderName" : "name")}
                </FieldLabel>
                <Input
                  ref={input}
                  id={`${id}-name`}
                  name="name"
                  placeholder={t(
                    operation.mode === "create" ? "folderName" : "name",
                  )}
                  maxLength={255}
                  value={name}
                  aria-invalid={error === "invalid" || undefined}
                  aria-describedby={error ? `${id}-error` : undefined}
                  onChange={(event) => {
                    setName(event.target.value);
                    setRequestId(crypto.randomUUID());
                    setError(null);
                  }}
                  autoFocus
                />
              </Field>
            </FieldGroup>
          ) : operation.mode === "move" ? (
            <FolderPicker
              source={source}
              initialParent={parentId}
              excluded={
                operation.entry.kind === "folder" ? operation.entry.id : null
              }
              onTarget={setTarget}
            />
          ) : (
            <>
              <p className="break-all">{operation.entry.name}</p>
              <p className="text-sm text-muted-foreground">
                {t(
                  operation.entry.kind === "folder"
                    ? "deleteFolderHint"
                    : "deleteFileHint",
                )}
              </p>
            </>
          )}
          {error ? (
            <FieldError id={`${id}-error`} role="alert">
              {t(error)}
            </FieldError>
          ) : null}
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={pending}
              autoFocus={deleting}
              onClick={onClose}
            >
              {t("cancel")}
            </Button>
            <Button
              type="submit"
              variant={deleting ? "destructive" : "default"}
              disabled={pending || (operation.mode === "move" && !target)}
            >
              {t(
                deleting
                  ? "confirmDelete"
                  : operation.mode === "create"
                    ? "create"
                    : operation.mode === "move"
                      ? "moveHere"
                      : "save",
              )}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
function FolderPicker({
  source,
  initialParent,
  excluded,
  onTarget,
}: {
  source: DriveTreeSource;
  initialParent: string;
  excluded: string | null;
  onTarget: (id: string) => void;
}) {
  const { t } = useDriveTreeTranslation();
  const [query, setQuery] = useState(() => ({
    ...initial(100),
    parentId: initialParent,
  }));
  const [data, setData] = useState<DriveTreeListing | null>(null);
  const [failed, setFailed] = useState(false);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    void revision;
    const cancel = new AbortController();
    setData(null);
    setFailed(false);
    onTarget("");
    void source
      .list({ ...query, foldersOnly: true }, cancel.signal, false)
      .then(
        (result) => {
          if (!cancel.signal.aborted) {
            setData(result);
            onTarget(result.current?.id ?? "root");
          }
        },
        () => {
          if (!cancel.signal.aborted) setFailed(true);
        },
      );
    return () => cancel.abort();
  }, [query, source, onTarget, revision]);
  const navigate = (id: string) =>
    setQuery((previous) => ({ ...previous, parentId: id, page: 1 }));
  const chain: DriveFolder[] = data
    ? [...data.ancestors, ...(data.current ? [data.current] : [])]
    : [];
  return (
    <div className="flex flex-col gap-3">
      <PageBreadcrumbs
        current={data?.current?.name ?? t("root")}
        ancestors={
          data?.current
            ? [
                {
                  id: "root",
                  label: t("root"),
                  href: "#drive",
                  onNavigate: () => navigate("root"),
                },
                ...chain.slice(0, -1).map((folder) => ({
                  id: folder.id,
                  label: folder.name,
                  href: `#drive/${folder.id}`,
                  onNavigate: () => navigate(folder.id),
                })),
              ]
            : []
        }
      />
      {data ? (
        <DataTable
          label={t("destination")}
          data={data.items.filter(
            (entry): entry is Extract<DriveEntry, { kind: "folder" }> =>
              entry.kind === "folder",
          )}
          columns={[
            {
              id: "name",
              label: t("name"),
              hideable: false,
              cell: (folder) => (
                <Button
                  type="button"
                  variant="ghost"
                  className="h-auto whitespace-normal text-start"
                  disabled={folder.id === excluded}
                  onClick={() => navigate(folder.id)}
                >
                  <FolderIcon data-icon="inline-start" aria-hidden="true" />
                  {folder.name}
                </Button>
              ),
            },
          ]}
          getRowLabel={(folder) => folder.name}
          selectable={false}
          pagination={{
            page: query.page,
            pageSize: query.pageSize,
            total: data.total,
            onChange: (page, pageSize) =>
              setQuery((previous) => ({ ...previous, page, pageSize })),
          }}
          emptyState={<p>{t("empty")}</p>}
        />
      ) : failed ? (
        <div role="alert">
          {t("loadFailed")}
          <Button
            type="button"
            variant="outline"
            onClick={() => setRevision((value) => value + 1)}
          >
            {t("retry")}
          </Button>
        </div>
      ) : (
        <LoadingState label={t("loading")} />
      )}
    </div>
  );
}
