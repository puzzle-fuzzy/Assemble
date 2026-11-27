import { Button } from "@assemble/ui/button";
import { DataTable, type DataTableColumn } from "@assemble/ui/data-table";
import type { DriveTreeLocation } from "@assemble/ui/drive-tree-browser";
import { usePageCorrection } from "@assemble/ui/hooks/use-url-state";
import { useTrashTranslation } from "@assemble/ui/lib/drive-trash-i18n";
import {
  type DriveTreeSource,
  isDriveTreeListing,
} from "@assemble/ui/lib/drive-tree";
import { useDriveTreeTranslation } from "@assemble/ui/lib/drive-tree-i18n";
import { useUiTranslation } from "@assemble/ui/lib/i18n";
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  type AccessFailure,
  type AdminAccount,
  type AdminDriveFile,
  type AdminPage,
  AdminRequestError,
  type AdminStorage,
  adminDriveDownloadPath,
  listPath,
  request,
} from "../lib/api";
import { useAdminTranslation } from "../lib/i18n";
import { RecordedTime } from "../lib/text";
import { useResource } from "../lib/use-resource";
import { DetailDialog, DetailFields } from "./detail-dialog";
import { DriveAccountPicker } from "./drive-account-picker";
import { DriveOwnerWorkspace } from "./drive-owner-workspace";
import { DrivePreviewDialog } from "./drive-preview";
import { LoadingContent, NoResults, RequestError } from "./feedback";
import { SearchForm, useListQuery } from "./list-controls";
import { StorageWarnings, storageBytes } from "./storage-overview";

const EMPTY_FILES: AdminDriveFile[] = [];

function DriveCapacity({ storage }: { storage: AdminStorage }) {
  const { t: trashText } = useTrashTranslation();
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
          {
            label: trashText("trashBytes"),
            value: format(storage.trashBytes ?? null),
          },
          {
            label: trashText("reclaimableBytes"),
            value: format(storage.reclaimableBytes ?? null),
          },
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
  onLocationChange,
}: {
  onAccessFailure: (failure: AccessFailure) => void;
  onLocationChange?: (location: DriveTreeLocation | null) => void;
}) {
  const { t, i18n } = useAdminTranslation();
  const { t: treeText } = useDriveTreeTranslation();
  const [owner, setOwner] = useState<AdminAccount | null>(null);
  const [picking, setPicking] = useState(false);
  const treeSource = useMemo<DriveTreeSource>(
    () => ({
      list: async (query, signal) => {
        const params = new URLSearchParams({
          accountId: owner?.id ?? "",
          parentId: query.parentId,
          q: query.q,
          page: String(query.page),
          pageSize: String(query.pageSize),
          foldersOnly: String(query.foldersOnly ?? false),
        });
        try {
          const value: unknown = await request(
            `/v1/admin/drive/tree?${params}`,
            signal ?? new AbortController().signal,
          );
          if (!isDriveTreeListing(value))
            throw new AdminRequestError("unavailable");
          return value;
        } catch (error) {
          if (
            error instanceof AdminRequestError &&
            (error.kind === "signedOut" || error.kind === "forbidden")
          )
            onAccessFailure(error.kind);
          throw error;
        }
      },
    }),
    [owner, onAccessFailure],
  );
  const updateLocation = useCallback(
    (location: DriveTreeLocation | null) => {
      if (!owner || !location) {
        onLocationChange?.(null);
        return;
      }
      onLocationChange?.({
        currentId: location.currentId,
        current: location.ancestors.length
          ? location.current
          : owner.displayName,
        ancestors: [
          {
            id: "admin-drive",
            label: treeText("root"),
            href: "#drive",
            onNavigate: () => setOwner(null),
          },
          ...location.ancestors.map((ancestor, index) =>
            index === 0 ? { ...ancestor, label: owner.displayName } : ancestor,
          ),
        ],
      });
    },
    [owner, onLocationChange, treeText],
  );
  useEffect(() => () => onLocationChange?.(null), [onLocationChange]);
  const query = useListQuery("drive");
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
  usePageCorrection(page, query.correctPage);
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
      cell: (file) => (
        <Button
          variant="ghost"
          className="h-auto whitespace-normal text-start"
          onClick={() =>
            setOwner({
              id: file.accountId,
              displayName: file.accountDisplayName,
              email: file.accountEmail,
              avatarUrl: null,
            })
          }
        >
          {file.accountDisplayName}
        </Button>
      ),
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
      {owner ? (
        <DriveOwnerWorkspace
          owner={owner}
          onAccessFailure={onAccessFailure}
          variant="management"
          key={owner.id}
          source={treeSource}
          defaultPageSize={20}
          onLocationChange={updateLocation}
          formatSize={format}
          formatDate={(date) => <RecordedTime value={date} />}
          fileActions={(file) => {
            const selected: AdminDriveFile = {
              id: file.id,
              name: file.name,
              size: file.size,
              mimeType: file.mimeType ?? null,
              createdAt: file.createdAt,
              accountId: owner.id,
              accountDisplayName: owner.displayName,
              accountEmail: owner.email,
            };
            return [
              {
                id: "preview",
                label: sharedText("filePreview.open"),
                onSelect: () => setPreview(selected),
              },
              {
                id: "details",
                label: t("table.viewDetails"),
                onSelect: () => setSelected(selected),
              },
              {
                id: "download",
                label: t("drive.download"),
                href: adminDriveDownloadPath(file.id),
                download: true,
              },
            ];
          }}
        />
      ) : (
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
            <>
              <Button variant="outline" onClick={() => setPicking(true)}>
                {treeText("browse")}
              </Button>
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
            </>
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
                  query.query
                    ? t("empty.noMatchesTitle")
                    : t("drive.emptyTitle")
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
      )}
      {picking ? (
        <DriveAccountPicker
          onClose={() => setPicking(false)}
          onPick={(account) => {
            setOwner(account);
            setPicking(false);
          }}
          onAccessFailure={onAccessFailure}
        />
      ) : null}
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
