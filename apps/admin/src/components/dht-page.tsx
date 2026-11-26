import { Badge } from "@assemble/ui/badge";
import { Button } from "@assemble/ui/button";
import { Checkbox } from "@assemble/ui/checkbox";
import {
  DataTable,
  type DataTableColumn,
  DataTableColumnFilter,
} from "@assemble/ui/data-table";
import { Field, FieldLabel } from "@assemble/ui/field";
import { Input } from "@assemble/ui/input";
import { Label } from "@assemble/ui/label";
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
import { useDhtResource } from "../lib/use-dht-resource";
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
  const [statusFilter, setStatusFilter] = useState("all");
  const [sourceFilter, setSourceFilter] = useState("all");
  const [autoRefresh, setAutoRefresh] = useState(true);
  const autoId = useId();
  const queryId = useId();
  const [selected, setSelected] = useState<string | null>(null);
  const [queryOpen, setQueryOpen] = useState(false);
  const [hash, setHash] = useState("");
  const [busy, setBusy] = useState(false);
  const [settling, setSettling] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const mutation = useRef<AbortController | null>(null);
  useEffect(() => () => mutation.current?.abort(), []);
  const status = useDhtResource<DhtStatus>(
    "/v1/admin/dht/status",
    onAccessFailure,
    settling ? 1000 : autoRefresh ? 30000 : null,
  );
  const parameters = new URLSearchParams({
    q: query.query,
    page: String(query.page),
    pageSize: String(query.pageSize),
    ...(statusFilter !== "all" ? { status: statusFilter } : {}),
    ...(sourceFilter !== "all" ? { source: sourceFilter } : {}),
  });
  const list = useDhtResource<AdminPage<DhtObservation>>(
    `/v1/admin/dht/observations?${parameters}`,
    onAccessFailure,
    autoRefresh && selected === null && !queryOpen ? 30000 : null,
  );
  const page = list.resource.phase === "ready" ? list.resource.value : null;
  const runtime =
    status.resource.phase === "ready" ? status.resource.value : null;
  useEffect(() => {
    if (
      runtime &&
      (runtime.running === (runtime.mode !== "stopped") ||
        runtime.lastError !== null)
    )
      setSettling(false);
  }, [runtime]);

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
        setSettling(true);
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
          className="h-auto max-w-full justify-start p-0 text-left whitespace-normal break-all"
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
        <Badge variant={row.status === "verified" ? "secondary" : "outline"}>
          {t(`statuses.${row.status}`)}
        </Badge>
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
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-2">
          {runtime && (
            <Badge variant="outline">{t(`modes.${runtime.mode}`)}</Badge>
          )}
          <Button
            size="sm"
            variant="outline"
            disabled={
              busy || !runtime?.enabled || runtime.mode === "collecting"
            }
            onClick={() => void mutate("control", { mode: "collecting" })}
          >
            {t("collect")}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={
              busy || !runtime?.enabled || runtime.mode !== "collecting"
            }
            onClick={() => void mutate("control", { mode: "draining" })}
          >
            {t("drain")}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={busy || !runtime || runtime.mode === "stopped"}
            onClick={() => void mutate("control", { mode: "stopped" })}
          >
            {t("stop")}
          </Button>
        </div>
        <div className="flex items-center gap-2">
          <Checkbox
            id={autoId}
            checked={autoRefresh}
            onCheckedChange={(value) => setAutoRefresh(value === true)}
          />
          <Label htmlFor={autoId}>{t("autoRefresh")}</Label>
        </div>
      </div>
      {status.resource.phase === "loading" ? (
        <LoadingContent />
      ) : status.resource.phase === "error" ? (
        <RequestError failure={status.resource.failure} retry={status.retry} />
      ) : (
        <RuntimeStatus status={status.resource.value} />
      )}
      {failure && (
        <p role="alert" className="text-sm text-destructive">
          {t(errorKey(failure))}
        </p>
      )}
      {(list.refreshFailed || status.refreshFailed) && (
        <p role="alert" className="text-sm text-destructive">
          {t("errors.unavailable")}
        </p>
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
            <Button
              variant="outline"
              size="sm"
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
              size="sm"
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
            {failure && (
              <p role="alert" className="text-sm text-destructive">
                {t(errorKey(failure))}
              </p>
            )}
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
        <p role="alert" className="text-destructive">
          {t(errorKey(status.lastError))}
        </p>
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
        <p role="alert">{t("capacity")}</p>
      )}
      <details className="rounded-lg border p-3">
        <summary className="cursor-pointer font-medium">
          {t("diagnostics")}
        </summary>
        <dl className="mt-4 grid gap-4 sm:grid-cols-3 xl:grid-cols-4">
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
        <p className="mt-4 text-xs text-muted-foreground">{t("processHint")}</p>
      </details>
      <p className="text-xs text-muted-foreground">{t("evidenceHint")}</p>
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
  const detail = useDhtResource<DhtDetails>(
    `/v1/admin/dht/observations/${hash}`,
    onAccessFailure,
    3000,
    (value) => ["pending", "fetching", "deferred"].includes(value.status),
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
              <Badge variant="outline">{t(`statuses.${value.status}`)}</Badge>
              <Button
                size="sm"
                variant="outline"
                onClick={() => {
                  void copy(value).then((success) =>
                    setCopyNotice(success ? "copied" : "copyFailed"),
                  );
                }}
              >
                {t("copy")}
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
            {detail.refreshFailed && (
              <p role="alert" className="text-sm text-destructive">
                {t("errors.unavailable")}
              </p>
            )}
          </>
        )
      )}
    </DetailDialog>
  );
}
