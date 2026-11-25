import { Badge } from "@assemble/ui/badge";
import { Button } from "@assemble/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@assemble/ui/card";
import { ArrowLeftIcon, RefreshCwIcon } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";
import {
  type AccessFailure,
  type AdminDevice,
  type AdminLogin,
  type AdminOverview,
  type AdminPage,
  type AdminUser,
  type AdminUserDetail,
  listPath,
} from "../lib/api";
import { RecordedTime } from "../lib/text";
import { useResource } from "../lib/use-resource";
import {
  AccountIdentity,
  DevicesTable,
  LoginsTable,
  UsersTable,
} from "./data-tables";
import { LoadingContent, NoResults, RequestError } from "./feedback";
import { PageControls, SearchForm, useListQuery } from "./list-controls";

interface AccessProps {
  onAccessFailure: (failure: AccessFailure) => void;
}

function PageDescription({ description }: { description: string }) {
  return <p className="text-sm text-muted-foreground">{description}</p>;
}

export function OverviewPage({ onAccessFailure }: AccessProps) {
  const { resource, retry } = useResource<AdminOverview>(
    "/v1/admin/overview",
    onAccessFailure,
  );
  return (
    <>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <PageDescription description="查看当前账户和设备的实际数量。" />
        <Button
          variant="outline"
          size="sm"
          onClick={retry}
          disabled={resource.phase === "loading"}
        >
          <RefreshCwIcon data-icon="inline-start" aria-hidden="true" />
          刷新
        </Button>
      </div>
      {resource.phase === "loading" ? (
        <LoadingContent />
      ) : resource.phase === "error" ? (
        <RequestError failure={resource.failure} retry={retry} />
      ) : (
        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
          {[
            {
              title: "注册用户",
              value: resource.value.users,
              description: "当前注册账户总数",
            },
            {
              title: "使用设备",
              value: resource.value.devices,
              description: "当前已登记的设备",
            },
            {
              title: "在线设备",
              value: resource.value.onlineDevices,
              description: "当前保持连接的设备",
            },
            {
              title: "今日注册",
              value: resource.value.registrationsToday,
              description: "按北京时间统计",
            },
          ].map((item) => (
            <Card key={item.title}>
              <CardHeader>
                <CardTitle>
                  <h2>{item.title}</h2>
                </CardTitle>
                <CardDescription>{item.description}</CardDescription>
              </CardHeader>
              <CardContent>
                <p className="text-3xl font-semibold tabular-nums">
                  {item.value.toLocaleString("zh-CN")}
                </p>
              </CardContent>
            </Card>
          ))}
        </div>
      )}
    </>
  );
}

interface CollectionProps<T> extends AccessProps {
  name: "users" | "devices" | "logins";
  description: string;
  searchLabel: string;
  placeholder: string;
  emptyTitle: string;
  emptyDescription: string;
  queryState: ReturnType<typeof useListQuery>;
  renderItems: (items: T[]) => ReactNode;
}

function Collection<T>({
  name,
  description,
  searchLabel,
  placeholder,
  emptyTitle,
  emptyDescription,
  queryState,
  renderItems,
  onAccessFailure,
}: CollectionProps<T>) {
  const { resource, retry } = useResource<AdminPage<T>>(
    listPath(name, queryState.query, queryState.page),
    onAccessFailure,
  );
  return (
    <>
      <PageDescription description={description} />
      <SearchForm
        label={searchLabel}
        placeholder={placeholder}
        {...queryState}
      />
      {resource.phase === "loading" ? (
        <LoadingContent />
      ) : resource.phase === "error" ? (
        <RequestError failure={resource.failure} retry={retry} />
      ) : (
        <>
          {resource.value.items.length === 0 ? (
            <NoResults
              title={queryState.query ? "没有匹配的结果" : emptyTitle}
              description={
                queryState.query
                  ? "换一个关键词，或清空搜索后再试。"
                  : emptyDescription
              }
            />
          ) : (
            renderItems(resource.value.items)
          )}
          <PageControls {...resource.value} setPage={queryState.setPage} />
        </>
      )}
    </>
  );
}

export function UsersPage({
  onAccessFailure,
  onDetailChange,
}: AccessProps & { onDetailChange?: (showing: boolean) => void }) {
  const queryState = useListQuery();
  const [selected, setSelected] = useState<string | null>(null);
  useEffect(() => {
    onDetailChange?.(selected !== null);
  }, [selected, onDetailChange]);
  if (selected !== null) {
    return (
      <UserDetail
        id={selected}
        onAccessFailure={onAccessFailure}
        onBack={() => setSelected(null)}
      />
    );
  }
  return (
    <div className="flex min-w-0 flex-col gap-4">
      <Collection<AdminUser>
        name="users"
        description="查看注册时间、最近登录和账户的使用设备。时间按北京时间显示。"
        searchLabel="搜索用户"
        placeholder="姓名或邮箱"
        emptyTitle="还没有注册用户"
        emptyDescription="新用户注册后会出现在这里。"
        queryState={queryState}
        onAccessFailure={onAccessFailure}
        renderItems={(items) => (
          <UsersTable users={items} onSelect={setSelected} />
        )}
      />
    </div>
  );
}

export function DevicesPage({ onAccessFailure }: AccessProps) {
  const queryState = useListQuery();
  return (
    <Collection<AdminDevice>
      name="devices"
      description="查看设备所属用户、平台、在线状态和使用时间。时间按北京时间显示。"
      searchLabel="搜索设备"
      placeholder="设备名称、用户姓名或邮箱"
      emptyTitle="还没有设备"
      emptyDescription="用户登录桌面端或助手入口后，已登记设备会出现在这里。"
      queryState={queryState}
      onAccessFailure={onAccessFailure}
      renderItems={(items) => <DevicesTable devices={items} />}
    />
  );
}

export function LoginsPage({ onAccessFailure }: AccessProps) {
  const queryState = useListQuery();
  return (
    <Collection<AdminLogin>
      name="logins"
      description="查看已记录的成功登录，时间按北京时间显示；历史中没有采集的时间不会补写。"
      searchLabel="搜索登录记录"
      placeholder="用户姓名或邮箱"
      emptyTitle="还没有登录记录"
      emptyDescription="之后发生的成功登录会记录在这里。"
      queryState={queryState}
      onAccessFailure={onAccessFailure}
      renderItems={(items) => <LoginsTable logins={items} />}
    />
  );
}

function UserDetail({
  id,
  onBack,
  onAccessFailure,
}: AccessProps & { id: string; onBack: () => void }) {
  const { resource, retry } = useResource<AdminUserDetail>(
    `/v1/admin/users/${encodeURIComponent(id)}`,
    onAccessFailure,
  );
  return (
    <>
      <div className="flex flex-col items-start gap-3">
        <Button variant="ghost" size="sm" onClick={onBack}>
          <ArrowLeftIcon data-icon="inline-start" aria-hidden="true" />
          返回用户列表
        </Button>
        <PageDescription description="账户信息、所属设备和最近的登录记录。" />
      </div>
      {resource.phase === "loading" ? (
        <LoadingContent />
      ) : resource.phase === "error" ? (
        <RequestError failure={resource.failure} retry={retry} />
      ) : (
        <>
          <Card>
            <CardHeader>
              <CardTitle>
                <h2>账户信息</h2>
              </CardTitle>
              <CardDescription>
                <span className="break-all">
                  账户编号：{resource.value.user.id}
                </span>
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              <AccountIdentity account={resource.value.user} />
              <div className="flex flex-wrap gap-2">
                <Badge
                  variant={
                    resource.value.user.isSuperAdmin ? "default" : "secondary"
                  }
                >
                  {resource.value.user.isSuperAdmin ? "超级管理员" : "用户"}
                </Badge>
                <Badge variant="outline">
                  {resource.value.user.hasGithub
                    ? "已关联 GitHub"
                    : "未关联 GitHub"}
                </Badge>
              </div>
              <dl className="grid gap-3 sm:grid-cols-2">
                <div>
                  <dt className="text-muted-foreground">注册时间</dt>
                  <dd>
                    <RecordedTime value={resource.value.user.createdAt} />
                  </dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">最近登录</dt>
                  <dd>
                    <RecordedTime value={resource.value.user.lastLoginAt} />
                  </dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">设备总数</dt>
                  <dd>{resource.value.user.deviceCount} 台</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">在线设备</dt>
                  <dd>{resource.value.user.onlineDeviceCount} 台</dd>
                </div>
              </dl>
            </CardContent>
          </Card>
          <div className="flex min-w-0 flex-col gap-2">
            <h2 className="text-base font-semibold">所属设备</h2>
            {resource.value.devices.length ? (
              <DevicesTable devices={resource.value.devices} />
            ) : (
              <NoResults
                title="这个账户还没有设备"
                description="登录桌面端或助手入口后会登记设备。"
              />
            )}
          </div>
          <div className="flex min-w-0 flex-col gap-2">
            <h2 className="text-base font-semibold">最近登录记录</h2>
            {resource.value.logins.length ? (
              <LoginsTable logins={resource.value.logins} />
            ) : (
              <NoResults
                title="这个账户还没有登录记录"
                description="历史中没有采集的登录不会补写。"
              />
            )}
          </div>
        </>
      )}
    </>
  );
}
