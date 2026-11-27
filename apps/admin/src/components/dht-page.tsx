import { Alert, AlertDescription } from "@assemble/ui/alert";
import { Button } from "@assemble/ui/button";
import {
  DataTable,
  type DataTableColumn,
  DataTableColumnFilter,
} from "@assemble/ui/data-table";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@assemble/ui/dropdown-menu";
import { Field, FieldError, FieldLabel } from "@assemble/ui/field";
import { Input } from "@assemble/ui/input";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@assemble/ui/tooltip";
import { ActivityIcon, ChevronDownIcon } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import {
  type AccessFailure,
  type AdminPage,
  AdminRequestError,
  type AdminSession,
} from "../lib/api";
import {
  type DhtDetails,
  type DhtMode,
  type DhtObservation,
  DhtRequestError,
  type DhtStatus,
  dhtWrite,
  metadataStatuses,
} from "../lib/dht-api";
import { useDhtTranslation } from "../lib/dht-i18n";
import { RecordedTime } from "../lib/text";
import { useResource } from "../lib/use-resource";
import { DetailDialog, DetailFields } from "./detail-dialog";
import { LoadingContent, RequestError } from "./feedback";
import { SearchForm, useListQuery } from "./list-controls";
import { storageBytes } from "./storage-overview";

const EMPTY_ROWS: DhtObservation[] = [];
const ERROR_CODES = new Set([
  "DHT_UNAVAILABLE",
  "DHT_RETRY_COOLDOWN",
  "DHT_QUEUE_FULL",
  "DHT_RECORD_LIMIT",
  "dht_bind_failed",
  "dht_persistence_failed",
  "metadata_worker_failed",
  "metadata_timeout",
  "metadata_connection",
  "metadata_protocol",
  "metadata_invalid",
  "metadata_unsupported",
  "metadata_rejected",
  "metadata_no_peers",
  "metadata_interrupted",
  "metadata_storage_full",
  "invalid",
  "csrfRejected",
]);
function errorKey(code: string) {
  return `errors.${ERROR_CODES.has(code) ? code : "unavailable"}`;
}

export function DhtPage({
  session,
  onAccessFailure,
}: {
  session: AdminSession;
  onAccessFailure: (failure: AccessFailure) => void;
}) {
  const { t } = useDhtTranslation();
  const query = useListQuery();
  const [statusFilter, setStatusFilter] = useState("verified");
  const [sourceFilter, setSourceFilter] = useState("all");
  const queryId = useId();
  const [runtimeOpen, setRuntimeOpen] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [queryOpen, setQueryOpen] = useState(false);
  const [hash, setHash] = useState("");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const mutation = useRef<AbortController | null>(null);
  useEffect(() => () => mutation.current?.abort(), []);
  const status = useResource<DhtStatus>(
    "/v1/admin/dht/status",
    onAccessFailure,
  );
  const parameters = new URLSearchParams({
    q: query.query,
    page: String(query.page),
    pageSize: String(query.pageSize),
    ...(statusFilter !== "all" ? { status: statusFilter } : {}),
    ...(sourceFilter !== "all" ? { source: sourceFilter } : {}),
  });
  const list = useResource<AdminPage<DhtObservation>>(
    `/v1/admin/dht/observations?${parameters}`,
    onAccessFailure,
  );
  const page = list.resource.phase === "ready" ? list.resource.value : null;
  const runtime =
    status.resource.phase === "ready" ? status.resource.value : null;

  async function mutate(
    action: "query" | "control",
    body: { query: string } | { mode: DhtMode },
  ) {
    if (mutation.current) return;
    const controller = new AbortController();
    mutation.current = controller;
    setBusy(true);
    setFailure(null);
    setNotice(null);
    try {
      const result = await dhtWrite<DhtDetails | { mode: DhtMode }>(
        action,
        body,
        session.csrfToken,
        controller.signal,
      );
      if (controller.signal.aborted) return;
      if (action === "query" && "id" in result) {
        setQueryOpen(false);
        setSelected(result.id);
        list.retry();
      }
      if (action === "control" && "mode" in result) {
        status.update((old) => ({ ...old, mode: result.mode }));
      }
    } catch (error) {
      if (controller.signal.aborted) return;
      if (
        error instanceof AdminRequestError &&
        (error.kind === "signedOut" || error.kind === "forbidden")
      ) {
        onAccessFailure(error.kind);
        return;
      }
      setFailure(
        error instanceof DhtRequestError
          ? error.kind === "csrfRejected"
            ? "csrfRejected"
            : error.kind === "invalid"
              ? "invalid"
              : error.code
          : "unavailable",
      );
    } finally {
      if (mutation.current === controller) {
        mutation.current = null;
        setBusy(false);
      }
    }
  }
  async function copy(row: DhtObservation) {
    try {
      await navigator.clipboard.writeText(row.magnetUri);
      setNotice("copied");
      return true;
    } catch {
      setNotice("copyFailed");
      setSelected(row.id);
      return false;
    }
  }
  function filter(kind: "status" | "source", value: string) {
    if (kind === "status") setStatusFilter(value);
    else setSourceFilter(value);
    query.setPagination(1, query.pageSize);
  }
  const columns: DataTableColumn<DhtObservation>[] = [
    {
      id: "name",
      label: t("name"),
      hideable: false,
      className: "min-w-56 max-w-96 whitespace-normal",
      cell: (row) => (
        <Button
          variant="link"
          className="h-auto min-h-8 max-w-full justify-start p-0 text-left whitespace-normal break-all"
          onClick={() => {
            setNotice(null);
            setSelected(row.id);
          }}
        >
          {row.name ?? row.id}
        </Button>
      ),
    },
    {
      id: "status",
      label: t("status"),
      header: (
        <DataTableColumnFilter
          label={t("status")}
          value={statusFilter}
          options={[
            { value: "all", label: t("all") },
            ...metadataStatuses.map((value) => ({
              value,
              label: t(`statuses.${value}`),
            })),
          ]}
          onValueChange={(value) => filter("status", value)}
        />
      ),
      cell: (row) => (
        <Button
          type="button"
          variant={row.status === "verified" ? "secondary" : "outline"}
          onClick={() => {
            setNotice(null);
            setSelected(row.id);
          }}
        >
          {t(`statuses.${row.status}`)}
        </Button>
      ),
    },
    {
      id: "size",
      label: t("size"),
      cell: (row) => <Bytes value={row.totalSize} />,
    },
    {
      id: "files",
      label: t("fileCount"),
      cell: (row) => row.fileCount ?? t("unknown"),
    },
    {
      id: "evidence",
      label: t("evidence"),
      header: (
        <DataTableColumnFilter
          label={t("evidence")}
          value={sourceFilter}
          options={[
            { value: "all", label: t("all") },
            { value: "announce", label: t("announce") },
            { value: "sample", label: t("sample") },
            { value: "query", label: t("querySource") },
          ]}
          onValueChange={(value) => filter("source", value)}
        />
      ),
      cell: (row) =>
        row.announceCount || row.sampleCount
          ? `${t("announce")} ${row.announceCount} · ${t("sample")} ${row.sampleCount}`
          : t("querySource"),
    },
    {
      id: "lastSeen",
      label: t("lastSeen"),
      cell: (row) => <RecordedTime value={row.lastSeen} />,
    },
  ];
  return (
    <div className="flex min-w-0 flex-col gap-4">
      {status.resource.phase === "error" && (
        <RequestError failure={status.resource.failure} retry={status.retry} />
      )}
      {failure && (
        <Alert variant="destructive">
          <AlertDescription>{t(errorKey(failure))}</AlertDescription>
        </Alert>
      )}
      <p
        role="status"
        className={notice ? "text-sm" : "sr-only"}
        aria-live="polite"
      >
        {notice ? t(notice) : ""}
      </p>
      <DataTable
        variant="management"
        label={t("region")}
        data={page?.items ?? EMPTY_ROWS}
        columns={columns}
        getRowLabel={(row) => row.name ?? row.id}
        toolbar={
          <SearchForm
            label={t("search")}
            placeholder={t("search")}
            {...query}
          />
        }
        actions={
          <>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="outline" disabled={!runtime || busy}>
                  {runtime ? t(`modes.${runtime.mode}`) : t("working")}
                  <ChevronDownIcon data-icon="inline-end" aria-hidden="true" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuGroup>
                  {(["collecting", "draining", "stopped"] as const).map(
                    (mode) => (
                      <DropdownMenuItem
                        key={mode}
                        disabled={
                          busy ||
                          !runtime ||
                          runtime.mode === mode ||
                          (mode !== "stopped" && !runtime.enabled) ||
                          (mode === "draining" && runtime.mode !== "collecting")
                        }
                        onSelect={() => void mutate("control", { mode })}
                      >
                        {t(
                          mode === "collecting"
                            ? "collect"
                            : mode === "draining"
                              ? "drain"
                              : "stop",
                        )}
                      </DropdownMenuItem>
                    ),
                  )}
                </DropdownMenuGroup>
              </DropdownMenuContent>
            </DropdownMenu>
            <TooltipProvider delayDuration={250}>
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant={runtime?.lastError ? "destructive" : "outline"}
                    size="icon"
                    aria-label={t("diagnostics")}
                    disabled={!runtime}
                    onClick={() => setRuntimeOpen(true)}
                  >
                    <ActivityIcon aria-hidden="true" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>{t("diagnostics")}</TooltipContent>
              </Tooltip>
            </TooltipProvider>
            <Button
              variant="outline"
              size="default"
              disabled={busy || !runtime?.enabled || runtime.mode === "stopped"}
              onClick={() => {
                setFailure(null);
                setHash("");
                setQueryOpen(true);
              }}
            >
              {t("query")}
            </Button>
            <Button
              variant="outline"
              size="default"
              disabled={
                list.resource.phase === "loading" ||
                status.resource.phase === "loading"
              }
              onClick={() => {
                list.retry();
                status.retry();
              }}
            >
              {t("refresh")}
            </Button>
          </>
        }
        pagination={{
          page: page?.page ?? query.page,
          pageSize: page?.pageSize ?? query.pageSize,
          total: page?.total ?? 0,
          pending: list.resource.phase === "loading",
          onChange: query.setPagination,
        }}
        rowActions={(row) => [
          {
            id: "details",
            label: t("details"),
            onSelect: () => setSelected(row.id),
          },
          { id: "copy", label: t("copy"), onSelect: () => void copy(row) },
          {
            id: "query",
            label: t("queryAgain"),
            disabled:
              busy ||
              !runtime?.enabled ||
              runtime.mode === "stopped" ||
              ["pending", "fetching", "verified"].includes(row.status),
            onSelect: () => void mutate("query", { query: row.id }),
          },
        ]}
        emptyState={
          list.resource.phase === "loading" ? (
            <LoadingContent />
          ) : list.resource.phase === "error" ? (
            <RequestError failure={list.resource.failure} retry={list.retry} />
          ) : (
            <p className="text-sm text-muted-foreground">
              {t(
                query.query || statusFilter !== "all" || sourceFilter !== "all"
                  ? "noMatches"
                  : "empty",
              )}
            </p>
          )
        }
      />
      {runtimeOpen && runtime && (
        <DetailDialog
          title={t("diagnostics")}
          onClose={() => setRuntimeOpen(false)}
        >
          <RuntimeStatus status={runtime} />
        </DetailDialog>
      )}
      {selected && (
        <ObservationDetail
          hash={selected}
          onAccessFailure={onAccessFailure}
          onClose={() => setSelected(null)}
          copy={copy}
        />
      )}
      {queryOpen && (
        <DetailDialog
          title={t("query")}
          onClose={() => {
            mutation.current?.abort();
            setQueryOpen(false);
            setFailure(null);
          }}
        >
          <form
            className="flex flex-col gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              void mutate("query", { query: hash });
            }}
          >
            <Field>
              <FieldLabel htmlFor={queryId}>{t("queryLabel")}</FieldLabel>
              <Input
                id={queryId}
                value={hash}
                onChange={(event) => setHash(event.target.value)}
                maxLength={4096}
                required
                autoComplete="off"
                spellCheck={false}
                aria-describedby={`${queryId}-hint`}
              />
              <p
                id={`${queryId}-hint`}
                className="text-sm text-muted-foreground"
              >
                {t("queryHint")}
              </p>
            </Field>
            {failure && <FieldError>{t(errorKey(failure))}</FieldError>}
            <Button type="submit" disabled={busy || !hash.trim()}>
              {t(busy ? "working" : "queryAction")}
            </Button>
          </form>
        </DetailDialog>
      )}
    </div>
  );
}

function Bytes({ value }: { value: number | null }) {
  const { t, i18n } = useDhtTranslation();
  return value === null
    ? t("unknown")
    : storageBytes(value, i18n.resolvedLanguage || i18n.language);
}

function RuntimeStatus({ status }: { status: DhtStatus }) {
  const { t } = useDhtTranslation();
  const queued =
    (status.statuses.pending ?? 0) + (status.statuses.fetching ?? 0);
  return (
    <section
      aria-label={t("diagnostics")}
      className="flex flex-col gap-3 text-sm"
    >
      {!status.enabled && (
        <p className="text-muted-foreground">{t("disabled")}</p>
      )}
      {status.enabled && status.running !== (status.mode !== "stopped") && (
        <p role="status">{t("starting")}</p>
      )}
      {status.lastError && (
        <Alert variant="destructive">
          <AlertDescription>{t(errorKey(status.lastError))}</AlertDescription>
        </Alert>
      )}
      <p>
        {t("totals", {
          total: status.total,
          verified: status.statuses.verified ?? 0,
          queued,
          limit: status.queueLimit,
          deferred: status.statuses.deferred ?? 0,
        })}
      </p>
      {(status.total >= status.maxRecords ||
        status.metadataBytes >= status.metadataBudgetBytes) && (
        <Alert>
          <AlertDescription>{t("capacity")}</AlertDescription>
        </Alert>
      )}
      <dl className="grid gap-4 sm:grid-cols-3 xl:grid-cols-4">
        {[
          [t("recordBudget"), status.maxRecords],
          [
            t("metadataBudget"),
            <span key="bytes">
              <Bytes value={status.metadataBytes} /> /{" "}
              <Bytes value={status.metadataBudgetBytes} />
            </span>,
          ],
          [t("workers"), status.workers],
          [t("announces"), status.announces],
          [t("samples"), status.samples],
          [
            t("lastObserved"),
            <RecordedTime key="time" value={status.lastObservedAt} />,
          ],
          ...Object.entries(status.counters).map(([key, count]) => [
            t(`counters.${key}`),
            count,
          ]),
        ].map(([label, value]) => (
          <div key={String(label)}>
            <dt className="text-muted-foreground">{label}</dt>
            <dd className="mt-1 tabular-nums">{value}</dd>
          </div>
        ))}
      </dl>
    </section>
  );
}

function ObservationDetail({
  hash,
  onAccessFailure,
  onClose,
  copy,
}: {
  hash: string;
  onAccessFailure: (failure: AccessFailure) => void;
  onClose: () => void;
  copy: (row: DhtObservation) => Promise<boolean>;
}) {
  const { t } = useDhtTranslation();
  const [copyNotice, setCopyNotice] = useState<string | null>(null);
  const detail = useResource<DhtDetails>(
    `/v1/admin/dht/observations/${hash}`,
    onAccessFailure,
  );
  const value =
    detail.resource.phase === "ready" ? detail.resource.value : null;
  const files =
    value?.files.map((file, index) => ({ id: String(index), ...file })) ?? [];
  return (
    <DetailDialog title={value?.name ?? t("details")} onClose={onClose}>
      {detail.resource.phase === "loading" ? (
        <LoadingContent />
      ) : detail.resource.phase === "error" ? (
        <RequestError failure={detail.resource.failure} retry={detail.retry} />
      ) : (
        value && (
          <>
            <div className="flex flex-wrap items-center gap-2">
              <Button
                size="default"
                variant="outline"
                onClick={() => {
                  void copy(value).then((success) =>
                    setCopyNotice(success ? "copied" : "copyFailed"),
                  );
                }}
              >
                {t("copy")}
              </Button>
              <Button variant="outline" onClick={detail.retry}>
                {t("refresh")}
              </Button>
            </div>
            <p
              role="status"
              aria-live="polite"
              className={copyNotice ? "text-sm" : "sr-only"}
            >
              {copyNotice ? t(copyNotice) : ""}
            </p>
            <DetailFields
              fields={[
                { label: t("status"), value: t(`statuses.${value.status}`) },
                {
                  label: t("hash"),
                  value: (
                    <code className="select-all break-all">{value.id}</code>
                  ),
                },
                { label: t("size"), value: <Bytes value={value.totalSize} /> },
                {
                  label: t("firstSeen"),
                  value: <RecordedTime value={value.firstSeen} />,
                },
                {
                  label: t("lastSeen"),
                  value: <RecordedTime value={value.lastSeen} />,
                },
                {
                  label: t("fetchedAt"),
                  value: <RecordedTime value={value.fetchedAt} />,
                },
                { label: t("attempts"), value: `${value.attempts} / 3` },
                {
                  label: t("pieceLength"),
                  value: <Bytes value={value.pieceLength} />,
                },
                {
                  label: t("private"),
                  value: t(
                    value.private === null
                      ? "unknown"
                      : value.private
                        ? "yes"
                        : "no",
                  ),
                },
                {
                  label: t("error"),
                  value: value.lastError
                    ? t(errorKey(value.lastError))
                    : t("unknown"),
                },
                { label: t("comment"), value: value.comment ?? t("unknown") },
                { label: t("source"), value: value.source ?? t("unknown") },
                {
                  label: t("magnet"),
                  value: (
                    <code className="select-all break-all">
                      {value.magnetUri}
                    </code>
                  ),
                },
              ]}
            />
            <DataTable
              label={t("files")}
              defaultPageSize={20}
              data={files}
              getRowLabel={(file) => file.path.join("/")}
              columns={[
                {
                  id: "path",
                  label: t("filePath"),
                  hideable: false,
                  className: "min-w-52 max-w-xl whitespace-normal break-all",
                  cell: (file) => file.path.join("/"),
                },
                {
                  id: "size",
                  label: t("size"),
                  cell: (file) => <Bytes value={file.length} />,
                },
              ]}
              emptyState={
                <p className="text-sm text-muted-foreground">
                  {t(`statuses.${value.status}`)}
                </p>
              }
            />
          </>
        )
      )}
    </DetailDialog>
  );
}
