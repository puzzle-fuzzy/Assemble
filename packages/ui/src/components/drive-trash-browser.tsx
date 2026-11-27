import { FolderIcon, XIcon } from "lucide-react";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { toast } from "sonner";
import type {
  TrashBatch,
  TrashDetail,
  TrashListing,
  TrashPreview,
  TrashRestored,
  TrashRestorePreview,
  TrashSource,
} from "../lib/drive-trash";
import { trashError, useTrashTranslation } from "../lib/drive-trash-i18n";
import type { DriveEntry, DriveTreeQuery } from "../lib/drive-tree";
import { Button } from "./button";
import { DataTable } from "./data-table";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./dialog";
import type { DriveTreeLocation } from "./drive-tree-browser";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "./input-group";
import { LoadingState } from "./loading-state";

type Operation =
  | { mode: "move"; entry: DriveEntry }
  | { mode: "restore" | "purge"; batch: TrashBatch };
interface Common {
  source: TrashSource;
  formatSize: (size: number) => string;
  formatDate: (date: string) => ReactNode;
}

export function TrashConfirm({
  source,
  operation,
  formatSize,
  onClose,
  onSaved,
}: {
  source: TrashSource;
  operation: Operation;
  formatSize: (size: number) => string;
  onClose: () => void;
  onSaved: (result?: TrashRestored) => void;
}) {
  const { t } = useTrashTranslation();
  const [preview, setPreview] = useState<
    TrashPreview | TrashRestorePreview | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const [pending, setPending] = useState(false);
  const busy = useRef(false);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const requestId = useRef(crypto.randomUUID());
  const destructive = operation.mode !== "restore";
  useEffect(() => {
    void revision;
    const cancel = new AbortController();
    setError(null);
    setPreview(null);
    if (operation.mode === "purge") return;
    const reading =
      operation.mode === "move"
        ? source.preview?.(
            {
              kind: operation.entry.kind,
              id: operation.entry.id,
              version: operation.entry.version,
            },
            cancel.signal,
          )
        : source.restorePreview?.(operation.batch.id, cancel.signal);
    if (!reading) {
      setError("unavailable");
      return;
    }
    void reading.then(
      (value) => {
        if (!cancel.signal.aborted) setPreview(value);
      },
      (cause) => {
        if (!cancel.signal.aborted) setError(trashError(cause));
      },
    );
    return () => cancel.abort();
  }, [source, operation, revision]);
  async function submit() {
    if (busy.current || (operation.mode !== "purge" && !preview)) return;
    busy.current = true;
    setPending(true);
    setError(null);
    try {
      let restored: TrashRestored | undefined;
      if (
        operation.mode === "move" &&
        preview &&
        "target" in preview &&
        source.move
      )
        await source.move({
          target: preview.target,
          revision: preview.revision,
          requestId: requestId.current,
        });
      else if (
        operation.mode === "restore" &&
        preview &&
        "version" in preview &&
        source.restore
      )
        restored = await source.restore(operation.batch.id, {
          version: preview.version,
          revision: preview.revision,
        });
      else if (operation.mode === "purge" && source.purge)
        await source.purge(operation.batch.id, operation.batch.version);
      else throw { code: "DRIVE_TRASH_UPGRADE_REQUIRED" };
      if (alive.current) onSaved(restored);
    } catch (cause) {
      if (alive.current) setError(trashError(cause));
    } finally {
      busy.current = false;
      if (alive.current) setPending(false);
    }
  }
  const restore = preview && "parentLocation" in preview ? preview : null;
  const summary =
    operation.mode === "move" && preview && "target" in preview
      ? preview
      : operation.mode === "purge"
        ? operation.batch
        : null;
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !pending) onClose();
      }}
    >
      <DialogContent aria-describedby={undefined}>
        <DialogHeader>
          <DialogTitle>
            {t(
              operation.mode === "move"
                ? "moveTitle"
                : operation.mode === "restore"
                  ? "restoreTitle"
                  : "purgeTitle",
            )}
          </DialogTitle>
        </DialogHeader>
        <p className="break-all">
          {operation.mode === "move"
            ? operation.entry.name
            : operation.batch.name}
        </p>
        {summary ? (
          <p>
            {t("summary", {
              files: t("fileCount", { count: summary.fileCount }),
              folders: t("folderCount", { count: summary.folderCount }),
              size: formatSize(summary.size),
            })}
          </p>
        ) : null}
        {restore ? (
          <dl className="flex flex-col gap-2">
            <dt className="text-sm text-muted-foreground">
              {t("destination")}
            </dt>
            <dd className="break-all">
              {restore.parentLocation.join(" / ") || t("root")}
            </dd>
            {restore.renamed ? (
              <>
                <dt className="text-sm text-muted-foreground">
                  {t("renamed")}
                </dt>
                <dd className="break-all">{restore.name}</dd>
              </>
            ) : null}
            {restore.reason !== "original" ? (
              <dd>{t(restore.reason)}</dd>
            ) : null}
          </dl>
        ) : null}
        {operation.mode !== "restore" ? (
          <p className="text-sm text-muted-foreground">
            {t(operation.mode === "purge" ? "purgeHint" : "moveHint")}
          </p>
        ) : null}
        {operation.mode !== "purge" && !preview && !error ? (
          <LoadingState label={t("loading")} />
        ) : null}
        {error ? (
          <div className="flex flex-col items-start gap-2">
            <p role="alert">{t(error)}</p>
            {operation.mode !== "purge" ? (
              <Button
                variant="outline"
                onClick={() => setRevision((v) => v + 1)}
              >
                {t("retry")}
              </Button>
            ) : null}
          </div>
        ) : null}
        <DialogFooter>
          <Button
            variant="outline"
            autoFocus={destructive}
            disabled={pending}
            onClick={onClose}
          >
            {t("cancel")}
          </Button>
          <Button
            variant={operation.mode === "purge" ? "destructive" : "default"}
            disabled={pending || (operation.mode !== "purge" && !preview)}
            onClick={() => void submit()}
          >
            {t(operation.mode)}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

interface Props extends Common {
  refreshKey?: unknown;
  variant?: "default" | "management";
  defaultPageSize?: number;
  onLocationChange?: (location: DriveTreeLocation | null) => void;
  onFiles: () => void;
  onRestored?: (restored: TrashRestored) => void;
}
export function DriveTrashBrowser({
  source,
  formatSize,
  formatDate,
  variant,
  defaultPageSize = 10,
  onLocationChange,
  onFiles,
  onRestored,
  refreshKey,
}: Props) {
  const { t } = useTrashTranslation();
  const [query, setQuery] = useState<DriveTreeQuery>({
    parentId: "root",
    q: "",
    page: 1,
    pageSize: defaultPageSize,
  });
  const [batchId, setBatchId] = useState<string | null>(null);
  const [data, setData] = useState<TrashListing | TrashDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const [operation, setOperation] = useState<Operation | null>(null);
  const searches = useRef(new Map<string, DriveTreeQuery>());
  const table = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const navigate = useCallback(
    (id: string | null, parentId = "root") => {
      searches.current.set(`${batchId ?? ""}:${query.parentId}`, query);
      setQuery(
        searches.current.get(`${id ?? ""}:${parentId}`) ?? {
          parentId,
          q: "",
          page: 1,
          pageSize: defaultPageSize,
        },
      );
      setBatchId(id);
      setData(null);
    },
    [batchId, query, defaultPageSize],
  );
  useEffect(() => {
    void revision;
    void refreshKey;
    const cancel = new AbortController();
    setLoading(true);
    setError(null);
    const read = batchId
      ? source.detail(batchId, query, cancel.signal)
      : source.list(query.q, query.page, query.pageSize, cancel.signal);
    void read.then(
      (value) => {
        if (!cancel.signal.aborted) {
          const last = Math.max(1, Math.ceil(value.total / query.pageSize));
          if (query.page > last) {
            setQuery((previous) => ({ ...previous, page: last }));
            return;
          }
          setData(value);
          setLoading(false);
        }
      },
      (cause) => {
        if (!cancel.signal.aborted) {
          setError(trashError(cause));
          setLoading(false);
        }
      },
    );
    return () => cancel.abort();
  }, [batchId, query, source, revision, refreshKey]);
  const detail = data && "batch" in data ? data : null;
  useEffect(() => {
    onLocationChange?.({
      currentId: detail
        ? `trash:${detail.batch.id}:${detail.current.id}`
        : "trash",
      current: detail?.current.name ?? t("trash"),
      ancestors: [
        {
          id: "drive-root",
          label: t("root"),
          href: "#drive",
          onNavigate: onFiles,
        },
        ...(detail
          ? [
              {
                id: "trash",
                label: t("trash"),
                href: "#drive/trash",
                onNavigate: () => navigate(null),
              },
              ...detail.ancestors.map((folder) => ({
                id: `trash:${folder.id}`,
                label: folder.name,
                href: `#drive/trash/${folder.id}`,
                onNavigate: () => navigate(detail.batch.id, folder.id),
              })),
            ]
          : []),
      ],
    });
  }, [detail, navigate, onFiles, onLocationChange, t]);
  useEffect(() => () => onLocationChange?.(null), [onLocationChange]);
  function close() {
    const id = operation && "batch" in operation ? operation.batch.id : null;
    setOperation(null);
    requestAnimationFrame(() => {
      (
        (id
          ? table.current?.querySelector<HTMLElement>(`[data-trash-id="${id}"]`)
          : null) ?? table.current
      )?.focus();
    });
  }
  const rows = detail
    ? detail.items.map((item) => ({
        id: item.id,
        name: item.name,
        kind: item.kind,
        size: item.size,
        date: item.createdAt,
        batch: null as TrashBatch | null,
      }))
    : (data?.items.map((item) => {
        const batch = item as TrashBatch;
        return {
          id: batch.id,
          name: batch.name,
          kind: batch.kind,
          size: batch.size,
          date: batch.deletedAt,
          batch,
        };
      }) ?? []);
  return (
    <div
      ref={table}
      tabIndex={-1}
      className="flex min-w-0 flex-col gap-3 outline-none"
    >
      <DataTable
        variant={variant}
        label={t("trash")}
        data={rows}
        selectable={false}
        getRowLabel={(row) => row.name}
        columns={[
          {
            id: "name",
            label: t("name"),
            hideable: false,
            className: "min-w-52 max-w-96 whitespace-normal break-all",
            cell: (row) =>
              row.kind === "folder" ? (
                <Button
                  data-trash-id={row.id}
                  variant="ghost"
                  className="h-auto whitespace-normal text-start"
                  onClick={() =>
                    navigate(
                      row.batch?.id ?? batchId,
                      row.batch ? "root" : row.id,
                    )
                  }
                >
                  <FolderIcon aria-hidden="true" data-icon="inline-start" />
                  {row.name}
                </Button>
              ) : (
                <span data-trash-id={row.id} tabIndex={-1}>
                  {row.name}
                </span>
              ),
          },
          {
            id: "location",
            label: t("originalLocation"),
            cell: (row) =>
              row.batch
                ? row.batch.originalLocation.join(" / ") || t("root")
                : "—",
          },
          {
            id: "size",
            label: t("size"),
            cell: (row) =>
              row.size === null ? t("folder") : formatSize(row.size),
          },
          ...(!detail
            ? [
                {
                  id: "contents",
                  label: t("contents"),
                  cell: (row: (typeof rows)[number]) =>
                    row.batch
                      ? t("counts", {
                          files: t("fileCount", { count: row.batch.fileCount }),
                          folders: t("folderCount", {
                            count: row.batch.folderCount,
                          }),
                        })
                      : "—",
                },
              ]
            : []),
          {
            id: "deletedAt",
            label: t("deletedAt"),
            cell: (row) =>
              formatDate(detail ? detail.batch.deletedAt : row.date),
          },
        ]}
        toolbar={
          <search>
            <form onSubmit={(event) => event.preventDefault()}>
              <InputGroup className="w-64 max-w-full">
                <InputGroupInput
                  ref={input}
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
                        input.current?.focus();
                      }}
                    >
                      <XIcon aria-hidden="true" />
                    </InputGroupButton>
                  </InputGroupAddon>
                ) : null}
              </InputGroup>
            </form>
          </search>
        }
        actions={
          <Button
            variant="outline"
            disabled={loading}
            onClick={() => setRevision((v) => v + 1)}
          >
            {t("refresh")}
          </Button>
        }
        rowActions={(row) =>
          row.batch
            ? [
                ...(source.restore
                  ? [
                      {
                        id: "restore",
                        label: t("restore"),
                        onSelect: () =>
                          setOperation({
                            mode: "restore",
                            batch: row.batch as TrashBatch,
                          }),
                      },
                    ]
                  : []),
                ...(source.purge
                  ? [
                      {
                        id: "purge",
                        label: t("purge"),
                        destructive: true,
                        onSelect: () =>
                          setOperation({
                            mode: "purge",
                            batch: row.batch as TrashBatch,
                          }),
                      },
                    ]
                  : []),
              ]
            : []
        }
        pagination={{
          page: query.page,
          pageSize: query.pageSize,
          total: data?.total ?? 0,
          pending: loading,
          onChange: (page, pageSize) =>
            setQuery((previous) => ({ ...previous, page, pageSize })),
        }}
        emptyState={
          loading ? (
            <LoadingState label={t("loading")} />
          ) : error ? (
            <div className="flex flex-col items-center gap-2">
              <p role="alert">{t(error)}</p>
              <Button
                variant="outline"
                onClick={() => setRevision((v) => v + 1)}
              >
                {t("retry")}
              </Button>
            </div>
          ) : (
            <p>{t("empty")}</p>
          )
        }
      />
      {error && data ? <p role="alert">{t(error)}</p> : null}
      {operation ? (
        <TrashConfirm
          source={source}
          operation={operation}
          formatSize={formatSize}
          onClose={close}
          onSaved={(restored) => {
            const mode = operation.mode;
            close();
            setRevision((v) => v + 1);
            toast.success(
              t(mode === "restore" ? "restored" : "purged"),
              restored && onRestored
                ? {
                    action: {
                      label: t("viewLocation"),
                      onClick: () => onRestored(restored),
                    },
                  }
                : undefined,
            );
          }}
        />
      ) : null}
    </div>
  );
}
