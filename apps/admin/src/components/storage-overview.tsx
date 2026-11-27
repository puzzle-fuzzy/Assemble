import { Card, CardContent, CardHeader, CardTitle } from "@assemble/ui/card";
import { DataTable, type DataTableColumn } from "@assemble/ui/data-table";
import { useTrashTranslation } from "@assemble/ui/lib/drive-trash-i18n";
import { Progress } from "@assemble/ui/progress";
import type { AdminStorage, AdminStorageAccount } from "../lib/api";
import { useOperationsTranslation } from "../lib/operations-i18n";

export function storageBytes(bytes: number, language: string) {
  const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
  const index = Math.max(
    0,
    Math.min(
      units.length - 1,
      Math.floor(Math.log(Math.max(1, bytes)) / Math.log(1024)),
    ),
  );
  return `${new Intl.NumberFormat(language, { maximumFractionDigits: index === 0 ? 0 : 1 }).format(bytes / 1024 ** index)} ${units[index]}`;
}

export function StorageWarnings({ storage }: { storage?: AdminStorage }) {
  const { t } = useOperationsTranslation();
  if (!storage || (!storage.budgetWarning && !storage.diskWarning)) return null;
  return (
    <div
      role="status"
      className={
        storage.diskWarning || storage.contentBytes >= storage.budgetBytes
          ? "text-sm text-destructive"
          : "text-sm text-warning"
      }
    >
      {storage.budgetWarning && (
        <p>{t("storage.budgetWarning", { percent: storage.warningPercent })}</p>
      )}
      {storage.diskWarning && <p>{t("storage.diskWarning")}</p>}
    </div>
  );
}

export function StorageOverview({ storage }: { storage?: AdminStorage }) {
  const { t: trashText } = useTrashTranslation();
  const { t, i18n } = useOperationsTranslation();
  const format = (bytes: number | null) =>
    bytes === null
      ? t("storage.unavailable")
      : storageBytes(bytes, i18n.resolvedLanguage || i18n.language);
  const columns: DataTableColumn<AdminStorageAccount>[] = [
    {
      id: "user",
      label: t("storage.user"),
      hideable: false,
      className: "min-w-44 whitespace-normal",
      cell: (account) => account.displayName,
    },
    {
      id: "email",
      label: t("storage.email"),
      className: "min-w-44 whitespace-normal break-all",
      cell: (account) => account.email ?? t("storage.emailMissing"),
    },
    {
      id: "used",
      label: t("storage.used"),
      cell: (account) => format(account.contentBytes),
    },
  ];
  return (
    <Card id="storage-overview" tabIndex={-1}>
      <CardHeader>
        <CardTitle>
          <h2>{t("storage.title")}</h2>
        </CardTitle>
      </CardHeader>
      <CardContent className="flex min-w-0 flex-col gap-6">
        {!storage ? (
          <p className="text-sm text-muted-foreground">
            {t("storage.unavailable")}
          </p>
        ) : (
          <>
            <dl className="grid gap-4 md:grid-cols-3">
              <div className="flex flex-col gap-2">
                <dt className="text-sm text-muted-foreground">
                  {t("storage.content")}
                </dt>
                <dd className="flex flex-col gap-2">
                  <span className="font-medium tabular-nums">
                    {t("storage.usage", {
                      used: format(storage.contentBytes),
                      budget: format(storage.budgetBytes),
                    })}
                  </span>
                  <Progress
                    aria-label={t("storage.content")}
                    aria-valuetext={t("storage.usage", {
                      used: format(storage.contentBytes),
                      budget: format(storage.budgetBytes),
                    })}
                    value={
                      storage.budgetBytes > 0
                        ? Math.min(
                            100,
                            (storage.contentBytes / storage.budgetBytes) * 100,
                          )
                        : 0
                    }
                  />
                </dd>
              </div>
              <div className="flex flex-col gap-2">
                <dt className="text-sm text-muted-foreground">
                  {t("storage.database")}
                </dt>
                <dd className="font-medium tabular-nums">
                  {format(storage.databaseBytes)}
                </dd>
              </div>
              <div className="flex flex-col gap-2">
                <dt className="text-sm text-muted-foreground">
                  {t("storage.diskAvailable")}
                </dt>
                <dd className="font-medium tabular-nums">
                  {format(storage.diskAvailableBytes)}
                </dd>
              </div>
              <div className="flex flex-col gap-2">
                <dt className="text-sm text-muted-foreground">
                  {trashText("trashBytes")}
                </dt>
                <dd className="font-medium tabular-nums">
                  {format(storage.trashBytes ?? null)}
                </dd>
              </div>
              <div className="flex flex-col gap-2">
                <dt className="text-sm text-muted-foreground">
                  {trashText("reclaimableBytes")}
                </dt>
                <dd className="font-medium tabular-nums">
                  {format(storage.reclaimableBytes ?? null)}
                </dd>
              </div>
            </dl>
            <StorageWarnings storage={storage} />
            <DataTable
              variant="management"
              defaultPageSize={20}
              label={t("storage.accounts")}
              data={storage.topAccounts}
              columns={columns}
              getRowLabel={(account) => account.displayName}
              emptyState={t("storage.noUsage")}
            />
          </>
        )}
      </CardContent>
    </Card>
  );
}
