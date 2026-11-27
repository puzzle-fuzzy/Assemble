import { Button } from "@assemble/ui/button";
import { DataTable } from "@assemble/ui/data-table";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@assemble/ui/dialog";
import { useDriveTreeTranslation } from "@assemble/ui/lib/drive-tree-i18n";
import type {
  AccessFailure,
  AdminAccount,
  AdminPage,
  AdminUser,
} from "../lib/api";
import { listPath } from "../lib/api";
import { useResource } from "../lib/use-resource";
import { LoadingContent, RequestError } from "./feedback";
import { SearchForm, useListQuery } from "./list-controls";

export function DriveAccountPicker({
  onClose,
  onPick,
  onAccessFailure,
}: {
  onClose: () => void;
  onPick: (account: AdminAccount) => void;
  onAccessFailure: (failure: AccessFailure) => void;
}) {
  const { t } = useDriveTreeTranslation();
  const query = useListQuery();
  const { resource, retry } = useResource<AdminPage<AdminUser>>(
    listPath("users", query.query, query.page, query.pageSize),
    onAccessFailure,
  );
  const page = resource.phase === "ready" ? resource.value : null;
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent
        className="max-h-[90svh] overflow-y-auto sm:max-w-3xl"
        aria-describedby={undefined}
      >
        <DialogHeader>
          <DialogTitle>{t("owner")}</DialogTitle>
        </DialogHeader>
        <DataTable
          variant="management"
          label={t("owner")}
          data={page?.items ?? []}
          columns={[
            {
              id: "name",
              label: t("name"),
              hideable: false,
              cell: (account) => (
                <Button
                  variant="ghost"
                  className="h-auto whitespace-normal text-start"
                  onClick={() => onPick(account)}
                >
                  {account.displayName}
                  {account.email ? ` · ${account.email}` : ""}
                </Button>
              ),
            },
          ]}
          getRowLabel={(account) => account.displayName}
          selectable={false}
          toolbar={
            <SearchForm
              label={t("owner")}
              placeholder={t("owner")}
              {...query}
            />
          }
          pagination={{
            page: query.page,
            pageSize: query.pageSize,
            total: page?.total ?? 0,
            pending: resource.phase === "loading",
            onChange: query.setPagination,
          }}
          emptyState={
            resource.phase === "loading" ? (
              <LoadingContent />
            ) : resource.phase === "error" ? (
              <RequestError failure={resource.failure} retry={retry} />
            ) : (
              <p>{t("empty")}</p>
            )
          }
        />
      </DialogContent>
    </Dialog>
  );
}
