import { Avatar, AvatarFallback, AvatarImage } from "@assemble/ui/avatar";
import { Button } from "@assemble/ui/button";
import {
  DataTable,
  type DataTableColumn,
  type DataTablePagination,
} from "@assemble/ui/data-table";
import { type ReactNode, useState } from "react";
import type {
  AdminAccount,
  AdminDevice,
  AdminLogin,
  AdminUser,
} from "../lib/api";
import { useAdminTranslation } from "../lib/i18n";
import { useOperationsTranslation } from "../lib/operations-i18n";
import { RecordedTime } from "../lib/text";
import { AccountStatus } from "./account-suspension";
import { DetailDialog, DetailFields } from "./detail-dialog";

export interface TableOptions {
  pagination?: DataTablePagination;
  toolbar?: ReactNode;
  actions?: ReactNode;
  emptyState?: ReactNode;
}

export function AccountIdentity({ account }: { account: AdminAccount }) {
  const { t } = useAdminTranslation();
  return (
    <span className="flex min-w-0 items-center gap-2">
      <Avatar>
        {account.avatarUrl && <AvatarImage src={account.avatarUrl} alt="" />}
        <AvatarFallback>{account.displayName.slice(0, 1)}</AvatarFallback>
      </Avatar>
      <span className="flex min-w-0 flex-col gap-0.5 text-left">
        <span className="break-all">{account.displayName}</span>
        <span className="break-all text-xs text-muted-foreground">
          {account.email ?? t("common.emailMissing")}
        </span>
      </span>
    </span>
  );
}

export function UsersTable({
  users,
  onSelect,
  ...options
}: TableOptions & { users: AdminUser[]; onSelect: (id: string) => void }) {
  const { t } = useAdminTranslation();
  const { t: operations } = useOperationsTranslation();
  const columns: DataTableColumn<AdminUser>[] = [
    {
      id: "user",
      label: t("table.user"),
      hideable: false,
      className: "min-w-32 max-w-60 whitespace-normal",
      cell: (user) => (
        <Button
          variant="link"
          className="h-auto min-h-8 justify-start p-0 whitespace-normal"
          onClick={() => onSelect(user.id)}
          aria-label={t("table.viewUserDetails", { name: user.displayName })}
        >
          {user.displayName}
        </Button>
      ),
    },
    {
      id: "email",
      label: t("table.email"),
      className: "min-w-44 max-w-72 whitespace-normal break-all",
      cell: (user) => user.email ?? t("common.emailMissing"),
    },
    {
      id: "createdAt",
      label: t("table.createdAt"),
      cell: (user) => <RecordedTime value={user.createdAt} />,
    },
    {
      id: "lastLoginAt",
      label: t("table.lastLoginAt"),
      cell: (user) => <RecordedTime value={user.lastLoginAt} />,
    },
    {
      id: "devices",
      label: t("table.devices"),
      cell: (user) =>
        t("table.deviceSummary", {
          count: user.deviceCount,
          online: user.onlineDeviceCount,
        }),
    },
    {
      id: "identity",
      label: t("table.identity"),
      cell: (user) =>
        [
          t(user.isSuperAdmin ? "common.superAdmin" : "common.user"),
          ...(user.hasGithub ? [t("common.github")] : []),
        ].join(" · "),
    },
    {
      id: "accountStatus",
      label: operations("account.status"),
      cell: (user) => <AccountStatus suspended={user.suspended} />,
    },
  ];
  return (
    <DataTable
      variant="management"
      defaultPageSize={20}
      {...options}
      label={t("table.usersRegion")}
      data={users}
      columns={columns}
      getRowLabel={(user) => user.displayName}
      rowActions={(user) => [
        { label: t("table.viewDetails"), onSelect: () => onSelect(user.id) },
      ]}
    />
  );
}

export function DevicesTable({
  devices,
  ...options
}: TableOptions & { devices: AdminDevice[] }) {
  const { t } = useAdminTranslation();
  const [selected, setSelected] = useState<AdminDevice | null>(null);
  const columns: DataTableColumn<AdminDevice>[] = [
    {
      id: "deviceName",
      label: t("table.deviceName"),
      hideable: false,
      className: "min-w-32 max-w-64 whitespace-normal break-all",
      cell: (device) => device.name,
    },
    {
      id: "deviceOwner",
      label: t("table.deviceOwner"),
      className: "min-w-40 max-w-64 whitespace-normal",
      cell: (device) => device.accountDisplayName,
    },
    {
      id: "platform",
      label: t("table.platform"),
      cell: (device) => t(`platform.${device.platform}`),
    },
    {
      id: "status",
      label: t("table.status"),
      cell: (device) => t(device.online ? "common.online" : "common.offline"),
    },
    {
      id: "deviceCreatedAt",
      label: t("table.deviceCreatedAt"),
      cell: (device) => <RecordedTime value={device.createdAt} />,
    },
    {
      id: "lastSeenAt",
      label: t("table.lastSeenAt"),
      cell: (device) => <RecordedTime value={device.lastSeenAt} />,
    },
  ];
  return (
    <>
      <DataTable
        variant="management"
        defaultPageSize={20}
        {...options}
        label={t("table.devicesRegion")}
        data={devices}
        columns={columns}
        getRowLabel={(device) => device.name}
        rowActions={(device) => [
          {
            label: t("table.viewDetails"),
            onSelect: () => setSelected(device),
          },
        ]}
      />
      {selected && (
        <DetailDialog
          title={t("recordDetail.device")}
          onClose={() => setSelected(null)}
        >
          <DetailFields
            fields={[
              { label: t("table.deviceName"), value: selected.name },
              { label: t("common.id"), value: selected.id },
              {
                label: t("table.deviceOwner"),
                value: selected.accountDisplayName,
              },
              {
                label: t("table.email"),
                value: selected.accountEmail ?? t("common.emailMissing"),
              },
              {
                label: t("table.platform"),
                value: t(`platform.${selected.platform}`),
              },
              {
                label: t("table.status"),
                value: t(selected.online ? "common.online" : "common.offline"),
              },
              {
                label: t("table.deviceCreatedAt"),
                value: <RecordedTime value={selected.createdAt} />,
              },
              {
                label: t("table.lastSeenAt"),
                value: <RecordedTime value={selected.lastSeenAt} />,
              },
            ]}
          />
        </DetailDialog>
      )}
    </>
  );
}

export function LoginsTable({
  logins,
  ...options
}: TableOptions & { logins: AdminLogin[] }) {
  const { t } = useAdminTranslation();
  const [selected, setSelected] = useState<AdminLogin | null>(null);
  const columns: DataTableColumn<AdminLogin>[] = [
    {
      id: "user",
      label: t("table.user"),
      hideable: false,
      className: "min-w-40 max-w-64 whitespace-normal",
      cell: (login) => login.displayName,
    },
    {
      id: "email",
      label: t("table.email"),
      className: "min-w-44 max-w-72 whitespace-normal break-all",
      cell: (login) => login.email ?? t("common.emailMissing"),
    },
    {
      id: "method",
      label: t("table.loginMethod"),
      cell: (login) => t(`loginMethod.${login.method}`),
    },
    {
      id: "occurredAt",
      label: t("table.occurredAt"),
      cell: (login) => <RecordedTime value={login.occurredAt} />,
    },
  ];
  return (
    <>
      <DataTable
        variant="management"
        defaultPageSize={20}
        {...options}
        label={t("table.loginsRegion")}
        data={logins}
        columns={columns}
        getRowLabel={(login) => login.displayName}
        rowActions={(login) => [
          { label: t("table.viewDetails"), onSelect: () => setSelected(login) },
        ]}
      />
      {selected && (
        <DetailDialog
          title={t("recordDetail.login")}
          onClose={() => setSelected(null)}
        >
          <DetailFields
            fields={[
              { label: t("common.id"), value: selected.id },
              { label: t("table.user"), value: selected.displayName },
              {
                label: t("table.email"),
                value: selected.email ?? t("common.emailMissing"),
              },
              {
                label: t("table.loginMethod"),
                value: t(`loginMethod.${selected.method}`),
              },
              {
                label: t("table.occurredAt"),
                value: <RecordedTime value={selected.occurredAt} />,
              },
            ]}
          />
        </DetailDialog>
      )}
    </>
  );
}
