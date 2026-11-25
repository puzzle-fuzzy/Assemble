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
import { platformName, RecordedTime } from "../lib/text";

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
  return (
    <div className="flex min-w-0 items-center gap-2">
      <Avatar>
        {account.avatarUrl && <AvatarImage src={account.avatarUrl} alt="" />}
        <AvatarFallback>{account.displayName.slice(0, 1)}</AvatarFallback>
      </Avatar>
      <div className="flex min-w-0 flex-col gap-0.5">
        <span className="break-all">{account.displayName}</span>
        <span className="break-all text-muted-foreground">
          {account.email ?? "未绑定邮箱"}
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
  return (
    <TableRegion title="注册用户表格，可横向滚动">
      <Table>
        <TableCaption className="sr-only">注册用户</TableCaption>
        <TableHeader>
          <TableRow>
            <TableHead scope="col">用户</TableHead>
            <TableHead scope="col">注册时间</TableHead>
            <TableHead scope="col">最近登录</TableHead>
            <TableHead scope="col">设备</TableHead>
            <TableHead scope="col">身份</TableHead>
            <TableHead scope="col">
              <span className="sr-only">操作</span>
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
                {user.deviceCount} 台 / {user.onlineDeviceCount} 在线
              </TableCell>
              <TableCell>
                <div className="flex flex-wrap gap-1">
                  <Badge variant={user.isSuperAdmin ? "default" : "secondary"}>
                    {user.isSuperAdmin ? "超级管理员" : "用户"}
                  </Badge>
                  {user.hasGithub && <Badge variant="outline">GitHub</Badge>}
                </div>
              </TableCell>
              <TableCell>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => onSelect(user.id)}
                  aria-label={`查看 ${user.displayName} 的详情`}
                >
                  查看详情
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
  return (
    <TableRegion title="设备表格，可横向滚动">
      <Table>
        <TableCaption className="sr-only">设备</TableCaption>
        <TableHeader>
          <TableRow>
            {[
              "设备名称",
              "所属用户",
              "平台",
              "状态",
              "添加时间",
              "最后在线",
            ].map((label) => (
              <TableHead key={label} scope="col">
                {label}
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
                    {device.accountEmail ?? "未绑定邮箱"}
                  </span>
                </div>
              </TableCell>
              <TableCell>{platformName[device.platform]}</TableCell>
              <TableCell>
                <Badge variant={device.online ? "default" : "secondary"}>
                  {device.online ? "在线" : "离线"}
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
  return (
    <TableRegion title="登录记录表格，可横向滚动">
      <Table>
        <TableCaption className="sr-only">登录记录</TableCaption>
        <TableHeader>
          <TableRow>
            <TableHead scope="col">用户</TableHead>
            <TableHead scope="col">登录方式</TableHead>
            <TableHead scope="col">登录时间</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {logins.map((login) => (
            <TableRow key={login.id}>
              <TableCell className="min-w-40 max-w-64 whitespace-normal">
                <div className="flex flex-col gap-0.5 break-all">
                  <span>{login.displayName}</span>
                  <span className="text-muted-foreground">
                    {login.email ?? "未绑定邮箱"}
                  </span>
                </div>
              </TableCell>
              <TableCell>
                <Badge variant="outline">
                  {login.method === "github" ? "GitHub" : "邮箱密码"}
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
