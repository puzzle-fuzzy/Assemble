import { Button } from "@assemble/ui/button";
import { DataTable, type DataTableColumn } from "@assemble/ui/data-table";
import { useUiTranslation } from "@assemble/ui/lib/i18n";
import { useState } from "react";
import {
  type AccessFailure,
  type AdminDriveFile,
  type AdminPage,
  type AdminStorage,
  adminDriveDownloadPath,
  listPath,
} from "../lib/api";
import { useAdminTranslation } from "../lib/i18n";
import { RecordedTime } from "../lib/text";
import { useResource } from "../lib/use-resource";
import { DetailDialog, DetailFields } from "./detail-dialog";
import { DrivePreviewDialog } from "./drive-preview";
import { LoadingContent, NoResults, RequestError } from "./feedback";
import { SearchForm, useListQuery } from "./list-controls";
import { StorageWarnings, storageBytes } from "./storage-overview";

const EMPTY_FILES: AdminDriveFile[] = [];

function DriveCapacity({ storage }: { storage: AdminStorage }) {
  const { t, i18n } = useAdminTranslation();
  const language = i18n.resolvedLanguage || i18n.language;
  const format = (value: number | null) =>
    value === null ? t("drive.unavailable") : storageBytes(value, language);
  const budgetRemaining = Math.max(
    0,
    storage.budgetBytes - storage.contentBytes,
  );
  // 只解释已报告的容量，不代替 Rust 的全站预算或单账户配额核验。
  const available =
    storage.diskAvailableBytes === null
      ? null
      : Math.min(budgetRemaining, storage.diskAvailableBytes);
  return (
    <section aria-label={t("drive.capacity")} className="flex flex-col gap-3">
      <StorageWarnings storage={storage} />
      <dl className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        {[
          { label: t("drive.available"), value: format(available) },
          {
            label: t("drive.diskAvailable"),
            value: format(storage.diskAvailableBytes),
          },
          {
            label: t("drive.contentUsage"),
            value: `${format(storage.contentBytes)} / ${format(storage.budgetBytes)}`,
          },
          { label: t("drive.database"), value: format(storage.databaseBytes) },
        ].map((item) => (
          <div key={item.label} className="flex min-w-0 flex-col gap-2">
            <dt className="text-sm text-muted-foreground">{item.label}</dt>
            <dd className="break-words text-xl font-semibold tabular-nums">
              {item.value}
            </dd>
          </div>
        ))}
      </dl>
    </section>
  );
}

export function DrivePage({
  onAccessFailure,
}: {
  onAccessFailure: (failure: AccessFailure) => void;
}) {
  const { t, i18n } = useAdminTranslation();
  const query = useListQuery();
  const files = useResource<AdminPage<AdminDriveFile>>(
    listPath("drive/files", query.query, query.page, query.pageSize),
    onAccessFailure,
  );
  const storage = useResource<AdminStorage>(
    "/v1/admin/storage",
    onAccessFailure,
  );
  const [selected, setSelected] = useState<AdminDriveFile | null>(null);
  const [preview, setPreview] = useState<AdminDriveFile | null>(null);
  const { t: sharedText } = useUiTranslation();
  const page = files.resource.phase === "ready" ? files.resource.value : null;
  const format = (bytes: number) =>
    storageBytes(bytes, i18n.resolvedLanguage || i18n.language);
  const columns: DataTableColumn<AdminDriveFile>[] = [
    {
      id: "name",
      label: t("drive.name"),
      hideable: false,
      className: "min-w-52 max-w-96 whitespace-normal break-all",
      cell: (file) => file.name,
    },
    {
      id: "owner",
      label: t("drive.owner"),
      className: "min-w-32 max-w-56 whitespace-normal break-all",
      cell: (file) => file.accountDisplayName,
    },
    {
      id: "email",
      label: t("table.email"),
      className: "min-w-44 max-w-72 whitespace-normal break-all",
      cell: (file) => file.accountEmail ?? t("common.emailMissing"),
    },
    { id: "size", label: t("drive.size"), cell: (file) => format(file.size) },
    {
      id: "mimeType",
      label: t("drive.mimeType"),
      className: "max-w-64 whitespace-normal break-all",
      cell: (file) => file.mimeType ?? t("common.unrecorded"),
    },
    {
      id: "createdAt",
      label: t("drive.createdAt"),
      cell: (file) => <RecordedTime value={file.createdAt} />,
    },
  ];
  return (
    <div className="flex min-w-0 flex-col gap-6">
      {storage.resource.phase === "ready" ? (
        <DriveCapacity storage={storage.resource.value} />
      ) : storage.resource.phase === "error" ? (
        <RequestError
          failure={
            storage.resource.failure === "notFound"
              ? "unavailable"
              : storage.resource.failure
          }
          retry={storage.retry}
        />
      ) : (
        <LoadingContent />
      )}
      <DataTable
        variant="management"
        label={t("drive.filesRegion")}
        data={page?.items ?? EMPTY_FILES}
        columns={columns}
        getRowLabel={(file) => file.name}
        toolbar={
          <SearchForm
            label={t("drive.searchLabel")}
            placeholder={t("drive.searchPlaceholder")}
            {...query}
          />
        }
        actions={
          <Button
            variant="outline"
            size="default"
            disabled={
              files.resource.phase === "loading" ||
              storage.resource.phase === "loading"
            }
            onClick={() => {
              files.retry();
              storage.retry();
            }}
          >
            {t("common.refresh")}
          </Button>
        }
        pagination={{
          page: page?.page ?? query.page,
          pageSize: page?.pageSize ?? query.pageSize,
          total: page?.total ?? 0,
          pending: files.resource.phase === "loading",
          onChange: query.setPagination,
        }}
        rowActions={(file) => [
          {
            id: "preview",
            label: sharedText("filePreview.open"),
            onSelect: () => setPreview(file),
          },
          {
            id: "details",
            label: t("table.viewDetails"),
            onSelect: () => setSelected(file),
          },
          {
            id: "download",
            label: t("drive.download"),
            href: adminDriveDownloadPath(file.id),
            download: true,
          },
        ]}
        emptyState={
          files.resource.phase === "loading" ? (
            <LoadingContent />
          ) : files.resource.phase === "error" ? (
            <RequestError
              failure={
                files.resource.failure === "notFound"
                  ? "unavailable"
                  : files.resource.failure
              }
              retry={files.retry}
            />
          ) : (
            <NoResults
              title={
                query.query ? t("empty.noMatchesTitle") : t("drive.emptyTitle")
              }
              description={
                query.query
                  ? t("empty.noMatchesDescription")
                  : t("drive.emptyDescription")
              }
            />
          )
        }
      />
      {preview && (
        <DrivePreviewDialog
          file={preview}
          onClose={() => setPreview(null)}
          onAccessFailure={onAccessFailure}
        />
      )}
      {selected && (
        <DetailDialog
          title={t("drive.details")}
          onClose={() => setSelected(null)}
        >
          <DetailFields
            fields={[
              { label: t("drive.name"), value: selected.name },
              { label: t("common.id"), value: selected.id },
              { label: t("drive.owner"), value: selected.accountDisplayName },
              {
                label: t("table.email"),
                value: selected.accountEmail ?? t("common.emailMissing"),
              },
              { label: t("drive.size"), value: format(selected.size) },
              {
                label: t("drive.mimeType"),
                value: selected.mimeType ?? t("common.unrecorded"),
              },
              {
                label: t("drive.createdAt"),
                value: <RecordedTime value={selected.createdAt} />,
              },
            ]}
          />
        </DetailDialog>
      )}
    </div>
  );
}
