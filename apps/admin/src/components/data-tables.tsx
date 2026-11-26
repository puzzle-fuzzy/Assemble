import { Avatar, AvatarFallback, AvatarImage } from "@assemble/ui/avatar";
import { Badge } from "@assemble/ui/badge";
import { Button } from "@assemble/ui/button";
import {
  DataTable,
  type DataTableColumn,
  type DataTablePagination,
} from "@assemble/ui/data-table";
import { CircleCheckIcon, CircleDashedIcon } from "lucide-react";
import type { ReactNode } from "react";
import type {
  AdminAccount,
  AdminDevice,
  AdminLogin,
  AdminUser,
} from "../lib/api";
import { useAdminTranslation } from "../lib/i18n";
import { RecordedTime } from "../lib/text";

export interface TableOptions {
  pagination?: DataTablePagination;
  toolbar?: ReactNode;
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
  const columns: DataTableColumn<AdminUser>[] = [
    {
      id: "user",
      label: t("table.user"),
      hideable: false,
      className: "min-w-44 max-w-72 whitespace-normal",
      cell: (user) => (
        <Button
          variant="link"
          className="h-auto justify-start p-0 whitespace-normal"
          onClick={() => onSelect(user.id)}
          aria-label={t("table.viewUserDetails", { name: user.displayName })}
        >
          <AccountIdentity account={user} />
        </Button>
      ),
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
      cell: (user) => (
        <div className="flex flex-wrap gap-1">
          <Badge variant="outline" className="px-1.5 text-muted-foreground">
            {t(user.isSuperAdmin ? "common.superAdmin" : "common.user")}
          </Badge>
          {user.hasGithub && (
            <Badge variant="outline" className="px-1.5 text-muted-foreground">
              {t("common.github")}
            </Badge>
          )}
        </div>
      ),
    },
  ];
  return (
    <DataTable
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
      cell: (device) => (
        <div className="flex flex-col gap-0.5 break-all">
          <span>{device.accountDisplayName}</span>
          <span className="text-xs text-muted-foreground">
            {device.accountEmail ?? t("common.emailMissing")}
          </span>
        </div>
      ),
    },
    {
      id: "platform",
      label: t("table.platform"),
      cell: (device) => (
        <Badge variant="outline" className="px-1.5 text-muted-foreground">
          {t(`platform.${device.platform}`)}
        </Badge>
      ),
    },
    {
      id: "status",
      label: t("table.status"),
      cell: (device) => (
        <Badge variant="outline" className="px-1.5 text-muted-foreground">
          {device.online ? (
            <CircleCheckIcon
              className="fill-success text-background"
              aria-hidden="true"
            />
          ) : (
            <CircleDashedIcon aria-hidden="true" />
          )}
          {t(device.online ? "common.online" : "common.offline")}
        </Badge>
      ),
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
    <DataTable
      {...options}
      label={t("table.devicesRegion")}
      data={devices}
      columns={columns}
      getRowLabel={(device) => device.name}
    />
  );
}

export function LoginsTable({
  logins,
  ...options
}: TableOptions & { logins: AdminLogin[] }) {
  const { t } = useAdminTranslation();
  const columns: DataTableColumn<AdminLogin>[] = [
    {
      id: "user",
      label: t("table.user"),
      hideable: false,
      className: "min-w-40 max-w-64 whitespace-normal",
      cell: (login) => (
        <div className="flex flex-col gap-0.5 break-all">
          <span>{login.displayName}</span>
          <span className="text-xs text-muted-foreground">
            {login.email ?? t("common.emailMissing")}
          </span>
        </div>
      ),
    },
    {
      id: "method",
      label: t("table.loginMethod"),
      cell: (login) => (
        <Badge variant="outline" className="px-1.5 text-muted-foreground">
          {t(`loginMethod.${login.method}`)}
        </Badge>
      ),
    },
    {
      id: "occurredAt",
      label: t("table.occurredAt"),
      cell: (login) => <RecordedTime value={login.occurredAt} />,
    },
  ];
  return (
    <DataTable
      {...options}
      label={t("table.loginsRegion")}
      data={logins}
      columns={columns}
      getRowLabel={(login) => login.displayName}
    />
  );
}
