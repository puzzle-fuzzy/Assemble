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
import { useAdminTranslation } from "../lib/i18n";
import { RecordedTime } from "../lib/text";
import { useResource } from "../lib/use-resource";
import {
  AccountIdentity,
  DevicesTable,
  LoginsTable,
  type TableOptions,
  UsersTable,
} from "./data-tables";
import { LoadingContent, NoResults, RequestError } from "./feedback";
import { SearchForm, useListQuery } from "./list-controls";

interface AccessProps {
  onAccessFailure: (failure: AccessFailure) => void;
}

export function OverviewPage({ onAccessFailure }: AccessProps) {
  const { t, i18n } = useAdminTranslation();
  const { resource, retry } = useResource<AdminOverview>(
    "/v1/admin/overview",
    onAccessFailure,
  );
  return (
    <>
      <div className="flex justify-end">
        <Button
          variant="outline"
          size="sm"
          onClick={retry}
          disabled={resource.phase === "loading"}
        >
          <RefreshCwIcon data-icon="inline-start" aria-hidden="true" />
          {t("common.refresh")}
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
              key: "users",
              value: resource.value.users,
            },
            {
              key: "devices",
              value: resource.value.devices,
            },
            {
              key: "onlineDevices",
              value: resource.value.onlineDevices,
            },
            {
              key: "registrationsToday",
              value: resource.value.registrationsToday,
            },
          ].map((item) => (
            <Card key={item.key}>
              <CardHeader>
                <CardTitle>
                  <h2
                    title={
                      item.key === "registrationsToday"
                        ? t("common.beijingTime")
                        : undefined
                    }
                  >
                    {t(`overview.${item.key}`)}
                  </h2>
                </CardTitle>
              </CardHeader>
              <CardContent>
                <p className="text-3xl font-semibold tabular-nums">
                  {item.value.toLocaleString(
                    i18n.resolvedLanguage || i18n.language,
                  )}
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
  searchLabel: string;
  placeholder: string;
  emptyTitle: string;
  emptyDescription: string;
  queryState: ReturnType<typeof useListQuery>;
  renderItems: (items: T[], options: TableOptions) => ReactNode;
}

const EMPTY_ROWS: never[] = [];
function Collection<T>({
  name,
  searchLabel,
  placeholder,
  emptyTitle,
  emptyDescription,
  queryState,
  renderItems,
  onAccessFailure,
}: CollectionProps<T>) {
  const { t } = useAdminTranslation();
  const { resource, retry } = useResource<AdminPage<T>>(
    listPath(name, queryState.query, queryState.page, queryState.pageSize),
    onAccessFailure,
  );
  const page = resource.phase === "ready" ? resource.value : null;
  const toolbar = (
    <SearchForm label={searchLabel} placeholder={placeholder} {...queryState} />
  );
  const emptyState =
    resource.phase === "loading" ? (
      <LoadingContent />
    ) : resource.phase === "error" ? (
      <RequestError failure={resource.failure} retry={retry} />
    ) : (
      <NoResults
        title={queryState.query ? t("empty.noMatchesTitle") : emptyTitle}
        description={
          queryState.query ? t("empty.noMatchesDescription") : emptyDescription
        }
      />
    );
  return renderItems(page?.items ?? EMPTY_ROWS, {
    toolbar,
    emptyState,
    pagination: {
      page: page?.page ?? queryState.page,
      pageSize: page?.pageSize ?? queryState.pageSize,
      total: page?.total ?? 0,
      pending: resource.phase === "loading",
      onChange: queryState.setPagination,
    },
  });
}

export function UsersPage({
  onAccessFailure,
  onDetailChange,
}: AccessProps & { onDetailChange?: (showing: boolean) => void }) {
  const { t } = useAdminTranslation();
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
        searchLabel={t("search.usersLabel")}
        placeholder={t("search.usersPlaceholder")}
        emptyTitle={t("empty.usersTitle")}
        emptyDescription={t("empty.usersDescription")}
        queryState={queryState}
        onAccessFailure={onAccessFailure}
        renderItems={(items, options) => (
          <UsersTable {...options} users={items} onSelect={setSelected} />
        )}
      />
    </div>
  );
}

export function DevicesPage({ onAccessFailure }: AccessProps) {
  const { t } = useAdminTranslation();
  const queryState = useListQuery();
  return (
    <Collection<AdminDevice>
      name="devices"
      searchLabel={t("search.devicesLabel")}
      placeholder={t("search.devicesPlaceholder")}
      emptyTitle={t("empty.devicesTitle")}
      emptyDescription={t("empty.devicesDescription")}
      queryState={queryState}
      onAccessFailure={onAccessFailure}
      renderItems={(items, options) => (
        <DevicesTable {...options} devices={items} />
      )}
    />
  );
}

export function LoginsPage({ onAccessFailure }: AccessProps) {
  const { t } = useAdminTranslation();
  const queryState = useListQuery();
  return (
    <Collection<AdminLogin>
      name="logins"
      searchLabel={t("search.loginsLabel")}
      placeholder={t("search.loginsPlaceholder")}
      emptyTitle={t("empty.loginsTitle")}
      emptyDescription={t("empty.loginsDescription")}
      queryState={queryState}
      onAccessFailure={onAccessFailure}
      renderItems={(items, options) => (
        <LoginsTable {...options} logins={items} />
      )}
    />
  );
}

function UserDetail({
  id,
  onBack,
  onAccessFailure,
}: AccessProps & { id: string; onBack: () => void }) {
  const { t } = useAdminTranslation();
  const { resource, retry } = useResource<AdminUserDetail>(
    `/v1/admin/users/${encodeURIComponent(id)}`,
    onAccessFailure,
  );
  return (
    <>
      <div className="flex flex-col items-start gap-3">
        <Button variant="ghost" size="sm" onClick={onBack}>
          <ArrowLeftIcon data-icon="inline-start" aria-hidden="true" />
          {t("userDetail.back")}
        </Button>
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
                <h2>{t("userDetail.accountInfo")}</h2>
              </CardTitle>
              <CardDescription>
                <span className="break-all">
                  {t("userDetail.accountId", { id: resource.value.user.id })}
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
                  {t(
                    resource.value.user.isSuperAdmin
                      ? "common.superAdmin"
                      : "common.user",
                  )}
                </Badge>
                <Badge variant="outline">
                  {t(
                    resource.value.user.hasGithub
                      ? "userDetail.githubLinked"
                      : "userDetail.githubUnlinked",
                  )}
                </Badge>
              </div>
              <dl className="grid gap-3 sm:grid-cols-2">
                <div>
                  <dt className="text-muted-foreground">
                    {t("userDetail.createdAt")}
                  </dt>
                  <dd>
                    <RecordedTime value={resource.value.user.createdAt} />
                  </dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">
                    {t("userDetail.lastLoginAt")}
                  </dt>
                  <dd>
                    <RecordedTime value={resource.value.user.lastLoginAt} />
                  </dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">
                    {t("userDetail.deviceCount")}
                  </dt>
                  <dd>
                    {t("userDetail.devices", {
                      count: resource.value.user.deviceCount,
                    })}
                  </dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">
                    {t("userDetail.onlineDeviceCount")}
                  </dt>
                  <dd>
                    {t("userDetail.devices", {
                      count: resource.value.user.onlineDeviceCount,
                    })}
                  </dd>
                </div>
              </dl>
            </CardContent>
          </Card>
          <div className="flex min-w-0 flex-col gap-2">
            <h2 className="text-base font-semibold">
              {t("userDetail.ownedDevices")}
            </h2>
            {resource.value.devices.length ? (
              <DevicesTable devices={resource.value.devices} />
            ) : (
              <NoResults
                title={t("empty.userDevicesTitle")}
                description={t("empty.userDevicesDescription")}
              />
            )}
          </div>
          <div className="flex min-w-0 flex-col gap-2">
            <h2 className="text-base font-semibold">
              {t("userDetail.recentLogins")}
            </h2>
            {resource.value.logins.length ? (
              <LoginsTable logins={resource.value.logins} />
            ) : (
              <NoResults
                title={t("empty.userLoginsTitle")}
                description={t("empty.userLoginsDescription")}
              />
            )}
          </div>
        </>
      )}
    </>
  );
}
