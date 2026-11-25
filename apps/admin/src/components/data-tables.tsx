import { Avatar, AvatarFallback, AvatarImage } from "@assemble/ui/avatar";
import { Badge } from "@assemble/ui/badge";
import { Button } from "@assemble/ui/button";
import {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@assemble/ui/table";
import { ChevronRightIcon } from "lucide-react";
import type { ReactNode } from "react";
import type {
  AdminAccount,
  AdminDevice,
  AdminLogin,
  AdminUser,
} from "../lib/api";
import { useAdminTranslation } from "../lib/i18n";
import { RecordedTime } from "../lib/text";

function TableRegion({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    // biome-ignore lint/a11y/noNoninteractiveTabindex: 具名表格区域需要让键盘用户横向滚动，遵循 responsive-table 指南。
    <section className="admin-table-region" aria-label={title} tabIndex={0}>
      {children}
    </section>
  );
}

export function AccountIdentity({ account }: { account: AdminAccount }) {
  const { t } = useAdminTranslation();
  return (
    <div className="flex min-w-0 items-center gap-2">
      <Avatar>
        {account.avatarUrl && <AvatarImage src={account.avatarUrl} alt="" />}
        <AvatarFallback>{account.displayName.slice(0, 1)}</AvatarFallback>
      </Avatar>
      <div className="flex min-w-0 flex-col gap-0.5">
        <span className="break-all">{account.displayName}</span>
        <span className="break-all text-muted-foreground">
          {account.email ?? t("common.emailMissing")}
        </span>
      </div>
    </div>
  );
}

export function UsersTable({
  users,
  onSelect,
}: {
  users: AdminUser[];
  onSelect: (id: string) => void;
}) {
  const { t } = useAdminTranslation();
  return (
    <TableRegion title={t("table.usersRegion")}>
      <Table>
        <TableCaption className="sr-only">
          {t("table.usersCaption")}
        </TableCaption>
        <TableHeader>
          <TableRow>
            <TableHead scope="col">{t("table.user")}</TableHead>
            <TableHead scope="col">{t("table.createdAt")}</TableHead>
            <TableHead scope="col">{t("table.lastLoginAt")}</TableHead>
            <TableHead scope="col">{t("table.devices")}</TableHead>
            <TableHead scope="col">{t("table.identity")}</TableHead>
            <TableHead scope="col">
              <span className="sr-only">{t("common.actions")}</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {users.map((user) => (
            <TableRow key={user.id}>
              <TableCell className="min-w-44 max-w-72 whitespace-normal">
                <AccountIdentity account={user} />
              </TableCell>
              <TableCell>
                <RecordedTime value={user.createdAt} />
              </TableCell>
              <TableCell>
                <RecordedTime value={user.lastLoginAt} />
              </TableCell>
              <TableCell>
                {t("table.deviceSummary", {
                  count: user.deviceCount,
                  online: user.onlineDeviceCount,
                })}
              </TableCell>
              <TableCell>
                <div className="flex flex-wrap gap-1">
                  <Badge variant={user.isSuperAdmin ? "default" : "secondary"}>
                    {t(user.isSuperAdmin ? "common.superAdmin" : "common.user")}
                  </Badge>
                  {user.hasGithub && (
                    <Badge variant="outline">{t("common.github")}</Badge>
                  )}
                </div>
              </TableCell>
              <TableCell>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => onSelect(user.id)}
                  aria-label={t("table.viewUserDetails", {
                    name: user.displayName,
                  })}
                >
                  {t("table.viewDetails")}
                  <ChevronRightIcon data-icon="inline-end" aria-hidden="true" />
                </Button>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </TableRegion>
  );
}

export function DevicesTable({ devices }: { devices: AdminDevice[] }) {
  const { t } = useAdminTranslation();
  return (
    <TableRegion title={t("table.devicesRegion")}>
      <Table>
        <TableCaption className="sr-only">
          {t("table.devicesCaption")}
        </TableCaption>
        <TableHeader>
          <TableRow>
            {[
              "deviceName",
              "deviceOwner",
              "platform",
              "status",
              "deviceCreatedAt",
              "lastSeenAt",
            ].map((label) => (
              <TableHead key={label} scope="col">
                {t(`table.${label}`)}
              </TableHead>
            ))}
          </TableRow>
        </TableHeader>
        <TableBody>
          {devices.map((device) => (
            <TableRow key={device.id}>
              <TableCell className="min-w-32 max-w-64 whitespace-normal break-all">
                {device.name}
              </TableCell>
              <TableCell className="min-w-40 max-w-64 whitespace-normal">
                <div className="flex flex-col gap-0.5 break-all">
                  <span>{device.accountDisplayName}</span>
                  <span className="text-muted-foreground">
                    {device.accountEmail ?? t("common.emailMissing")}
                  </span>
                </div>
              </TableCell>
              <TableCell>{t(`platform.${device.platform}`)}</TableCell>
              <TableCell>
                <Badge variant={device.online ? "default" : "secondary"}>
                  {t(device.online ? "common.online" : "common.offline")}
                </Badge>
              </TableCell>
              <TableCell>
                <RecordedTime value={device.createdAt} />
              </TableCell>
              <TableCell>
                <RecordedTime value={device.lastSeenAt} />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </TableRegion>
  );
}

export function LoginsTable({ logins }: { logins: AdminLogin[] }) {
  const { t } = useAdminTranslation();
  return (
    <TableRegion title={t("table.loginsRegion")}>
      <Table>
        <TableCaption className="sr-only">
          {t("table.loginsCaption")}
        </TableCaption>
        <TableHeader>
          <TableRow>
            <TableHead scope="col">{t("table.user")}</TableHead>
            <TableHead scope="col">{t("table.loginMethod")}</TableHead>
            <TableHead scope="col">{t("table.occurredAt")}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {logins.map((login) => (
            <TableRow key={login.id}>
              <TableCell className="min-w-40 max-w-64 whitespace-normal">
                <div className="flex flex-col gap-0.5 break-all">
                  <span>{login.displayName}</span>
                  <span className="text-muted-foreground">
                    {login.email ?? t("common.emailMissing")}
                  </span>
                </div>
              </TableCell>
              <TableCell>
                <Badge variant="outline">
                  {t(`loginMethod.${login.method}`)}
                </Badge>
              </TableCell>
              <TableCell>
                <RecordedTime value={login.occurredAt} />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </TableRegion>
  );
}
