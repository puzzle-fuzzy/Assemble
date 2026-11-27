// 应用主界面。
//
// 各功能共享窗口布局；账户切换统一隔离查询、草稿与异步结果。

import { useDisplayPreferences } from "@assemble/display";
import type { MediaPageLocation } from "@assemble/media/components/workspace";
import { Alert, AlertAction, AlertDescription } from "@assemble/ui/alert";
import { FieldError } from "@assemble/ui/field";
import { LoadingState } from "@assemble/ui/loading-state";
import { PageBreadcrumbs } from "@assemble/ui/page-breadcrumbs";
import { ArrowDownToLine, ArrowUpToLine, UserRound } from "lucide-react";
import { Fragment, lazy, Suspense, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { AssistantDownloadsSetting } from "@/AssistantDownloadsSetting";
import {
  DesktopMobileMenu,
  DesktopSidebar,
  desktopModules as SECTIONS,
  type DesktopModule as Section,
} from "@/components/desktop-sidebar";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { SidebarProvider } from "@/components/ui/sidebar";
import { Toaster } from "@/components/ui/sonner";
import { Switch } from "@/components/ui/switch";
import { DownloadsPage } from "@/DownloadsPage";
import { DriveRenameAction } from "@/DriveRenameDialog";
import { useSnapshot } from "@/hooks/use-snapshot";
import {
  type Bridge,
  bridge,
  type MediaBridge,
  mediaBridge,
  memoBridge,
  remoteBridge,
  toBridgeError,
} from "@/lib/bridge";
import { formatBuildInfo } from "@/lib/build-info";
import {
  formatStoredDate,
  platformName,
  uiText,
  useDesktopTranslation,
} from "@/lib/i18n";
import { memoText } from "@/lib/memo-i18n";
import type { MemoBridge } from "@/lib/memos";
import { messageFor } from "@/lib/messages";
import { noticeText } from "@/lib/notice-text";
import type { RemoteBridge } from "@/lib/remote";
import { remoteText } from "@/lib/remote-i18n";
import {
  formatSize,
  formatSpace,
  isSavedHere,
  transferStatus,
} from "@/lib/transfer-text";
import type {
  DeviceView,
  DriveFileView,
  LaunchState,
  Notice,
  ServiceView,
  Snapshot,
  TransferView,
} from "@/lib/types";
import { updateStatus } from "@/lib/update-text";
import { cn } from "@/lib/utils";
import { RemoteBanner, RemotePage } from "@/RemotePage";

const MemosPage = lazy(() =>
  import("@/MemosPage").then((module) => ({ default: module.MemosPage })),
);

const MediaPage = lazy(() =>
  import("@/MediaPage").then((module) => ({ default: module.MediaPage })),
);

import type { SystemNotification } from "@assemble/ui/lib/system-notifications";
import { PlannerCard } from "@/PlannerCard";
import { DesktopSystemNotifications } from "@/SystemNotifications";

export interface AppProps {
  /** 默认使用真实的桥接；测试时注入替身。 */
  bridge?: Bridge;
  mediaBridge?: MediaBridge;
  memoBridge?: MemoBridge;
  remoteBridge?: RemoteBridge;
}

const LOGIN_TIMEOUT_MESSAGE = "等待浏览器登录超时，请重试。";

export function App({
  bridge: source = bridge,
  mediaBridge: mediaSource = mediaBridge,
  memoBridge: memoSource = memoBridge,
  remoteBridge: remoteSource = remoteBridge,
}: AppProps) {
  useDesktopTranslation();
  const { resolvedTheme } = useDisplayPreferences();
  const { snapshot, loadError } = useSnapshot(source);
  const [accountPending, setAccountPending] = useState(false);
  const accountAction = useRef(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [section, setSection] = useState<Section>("transfers");
  const [mediaLocation, setMediaLocation] = useState<MediaPageLocation | null>(
    null,
  );
  const [queries, setQueries] = useState<Partial<Record<Section, string>>>({});
  const contentHeading = useRef<HTMLSpanElement>(null);
  const [systemFocus, setSystemFocus] = useState<{
    section: Section;
    id: string | null;
    revision: number;
  } | null>(null);
  useEffect(() => {
    if (!systemFocus || section !== systemFocus.section) return;
    const target = systemFocus.id
      ? document.getElementById(systemFocus.id)
      : null;
    (target ?? contentHeading.current)?.focus({ preventScroll: true });
    target?.scrollIntoView?.({ block: "nearest" });
  }, [systemFocus, section]);
  const phase = snapshot?.session.phase;
  const accountId =
    snapshot?.session.phase === "signedIn" ? snapshot.session.account.id : null;
  const currentMediaLocation =
    section === "media" && mediaLocation?.accountKey === accountId
      ? mediaLocation
      : null;
  const previousAccount = useRef(accountId);
  const previousSection = useRef(section);
  const compose = snapshot?.planner?.compose ?? null;
  const focused = snapshot?.planner?.focus ?? null;
  const browserTaskId = snapshot?.downloads.browserTaskId ?? null;
  useEffect(() => {
    if (browserTaskId !== null) setSection("downloads");
  }, [browserTaskId]);

  useEffect(() => {
    if (previousAccount.current === accountId) return;
    previousAccount.current = accountId;
    setQueries({});
    setActionError(null);
  }, [accountId]);

  // 托盘与系统通知各自响应变化；普通状态刷新不会抢走当前页面。
  useEffect(() => {
    if (compose === null) return;
    setSection("planner");
    setQueries((previous) => ({ ...previous, planner: "" }));
  }, [compose]);
  useEffect(() => {
    if (focused === null) return;
    setSection("planner");
    setQueries((previous) => ({ ...previous, planner: "" }));
  }, [focused]);

  const sectionLabel = SECTIONS().find((item) => item.id === section)?.label;
  useEffect(() => {
    document.title = `${currentMediaLocation ? `${currentMediaLocation.title} · ` : ""}${sectionLabel} · Assemble`;
    if (previousSection.current !== section) contentHeading.current?.focus();
    previousSection.current = section;
  }, [section, sectionLabel, currentMediaLocation]);

  function navigate(next: Section) {
    setSection(next);
    setActionError(null);
    // Rust 确认清除入口后，同一条通知或同类托盘操作才可再次触发定位。
    if (next !== "planner") {
      if (compose !== null)
        void source
          .plannerCompose(null)
          .catch((error: unknown) => setActionError(toBridgeError(error).code));
      if (focused !== null)
        void source
          .plannerFocus(null)
          .catch((error: unknown) => setActionError(toBridgeError(error).code));
    }
  }

  async function signOut() {
    if (accountAction.current) return;
    accountAction.current = true;
    setAccountPending(true);
    setActionError(null);
    try {
      await source.signOut();
    } catch (error: unknown) {
      setActionError(toBridgeError(error).code);
    } finally {
      accountAction.current = false;
      setAccountPending(false);
    }
  }

  function openSystemNotification(item: SystemNotification) {
    const content = item.content;
    const next: Section =
      content.type === "downloadFailed"
        ? "downloads"
        : content.type === "transferFailed"
          ? "transfers"
          : "account";
    setQueries((previous) => ({ ...previous, [next]: "" }));
    navigate(next);
    setSystemFocus((previous) => ({
      section: next,
      id:
        content.type === "transferFailed"
          ? `transfer-${content.transferId}`
          : content.type === "downloadFailed"
            ? `network-download-${content.taskId}`
            : null,
      revision: (previous?.revision ?? 0) + 1,
    }));
  }

  return (
    <>
      <Toaster
        theme={resolvedTheme}
        closeButton
        containerAriaLabel={uiText("actionNotifications")}
      />
      <SidebarProvider className="h-dvh min-h-0 items-stretch">
        <DesktopSidebar
          section={section}
          onNavigate={navigate}
          session={snapshot?.session}
          pending={accountPending}
          onSignOut={() => void signOut()}
          onNavigateFocus={() => contentHeading.current?.focus()}
        />
        <main className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
          <header className="flex h-12 shrink-0 items-center justify-between gap-2">
            <div className="flex min-w-0 items-center gap-2 px-4">
              <DesktopMobileMenu
                section={section}
                onNavigate={navigate}
                onNavigateFocus={() => contentHeading.current?.focus()}
              />
              <PageBreadcrumbs
                ancestors={[
                  {
                    id: "app",
                    label: "Assemble",
                    href: "#transfers",
                    onNavigate: () => navigate("transfers"),
                    hideOnNarrow: true,
                  },
                  ...(currentMediaLocation
                    ? [
                        {
                          id: "media",
                          label: sectionLabel ?? "",
                          href: "#media",
                          onNavigate: currentMediaLocation.showList,
                        },
                      ]
                    : []),
                ]}
                current={currentMediaLocation?.title ?? sectionLabel ?? ""}
                currentRef={contentHeading}
              />
            </div>
            <div className="flex min-w-0 shrink-0 items-center gap-2 px-4">
              <DesktopSystemNotifications
                key={accountId ?? "local"}
                source={source}
                view={snapshot?.systemNotifications}
                onNavigate={openSystemNotification}
                canNavigate={(item) =>
                  item.content.type === "sessionExpired" ||
                  (item.content.type === "transferFailed" &&
                    (snapshot?.transfers.some(
                      (transfer) =>
                        item.content.type === "transferFailed" &&
                        transfer.id === item.content.transferId,
                    ) ??
                      false)) ||
                  (item.content.type === "downloadFailed" &&
                    (snapshot?.downloads.tasks.some(
                      (task) =>
                        item.content.type === "downloadFailed" &&
                        task.id === item.content.taskId,
                    ) ??
                      false))
                }
              />
              <ServiceStatus
                service={snapshot?.service}
                signedIn={phase === "signedIn"}
              />
            </div>
          </header>
          <ScrollArea className="min-h-0 min-w-0 flex-1 [&>[data-slot=scroll-area-viewport]]:overscroll-y-contain">
            {/* Card 的外描边需要留在滚动区内，避免顶部贴边时被视口裁掉。 */}
            <div className="@container mx-auto flex w-full min-w-0 flex-col gap-4 p-4 pt-1">
              {loadError ? (
                <Alert variant="destructive">
                  <AlertDescription>{loadError}</AlertDescription>
                </Alert>
              ) : null}
              <RemoteBanner
                key={accountId ?? "remote-signedOut"}
                source={remoteSource}
                view={snapshot?.remote ?? { sessions: [] }}
                ownId={
                  snapshot?.session.phase === "signedIn"
                    ? snapshot.session.device.id
                    : null
                }
                onOpen={() => navigate("remote")}
              />
              {snapshot?.notice ? (
                <NoticeLine
                  notice={snapshot.notice}
                  onDismiss={() => void dismiss(source)}
                />
              ) : null}
              {actionError ? (
                <Alert variant="destructive">
                  <AlertDescription>
                    {messageFor(toBridgeError({ code: actionError }))}
                  </AlertDescription>
                </Alert>
              ) : null}
              {phase !== "signedIn" &&
              section !== "media" &&
              section !== "settings" &&
              section !== "downloads" &&
              section !== "account" &&
              section !== "about" ? (
                <Empty className="min-h-64">
                  <EmptyHeader>
                    <EmptyMedia variant="icon">
                      <UserRound aria-hidden="true" />
                    </EmptyMedia>
                    <EmptyTitle>
                      {snapshot
                        ? uiText("signInToAssemble")
                        : uiText("loadingDeviceStatusEllipsis")}
                    </EmptyTitle>
                    <EmptyDescription>
                      {uiText("completeSignInInYourSystemBrowser")}
                    </EmptyDescription>
                  </EmptyHeader>
                  <EmptyContent>
                    <Button
                      variant="outline"
                      onClick={() => navigate("account")}
                    >
                      {uiText("openAccountCenter")}
                    </Button>
                  </EmptyContent>
                </Empty>
              ) : null}
              {/* 只隐藏非当前页面，同一账户的草稿与进行中的操作继续保留。 */}
              <section
                id="view-downloads"
                aria-label={
                  SECTIONS().find((item) => item.id === "downloads")?.label
                }
                hidden={section !== "downloads"}
              >
                <DownloadsPage source={source} view={snapshot?.downloads} />
              </section>
              <section
                id="view-transfers"
                aria-label={uiText("fileTransfers")}
                hidden={section !== "transfers"}
              >
                <TransfersCard
                  key={accountId ?? "signedOut"}
                  source={source}
                  snapshot={snapshot}
                  onError={setActionError}
                  query={queries.transfers ?? ""}
                  onQueryChange={(value) =>
                    setQueries((previous) => ({
                      ...previous,
                      transfers: value,
                    }))
                  }
                />
              </section>
              <section
                id="view-drive"
                aria-label={uiText("cloudDrive")}
                hidden={section !== "drive"}
              >
                <DriveCard
                  key={accountId ?? "signedOut"}
                  source={source}
                  snapshot={snapshot}
                  onError={setActionError}
                  query={queries.drive ?? ""}
                  onQueryChange={(value) =>
                    setQueries((previous) => ({ ...previous, drive: value }))
                  }
                />
              </section>
              <section
                id="view-planner"
                aria-label={uiText("schedulesAndReminders")}
                hidden={section !== "planner"}
              >
                <PlannerCard
                  key={accountId ?? "signedOut"}
                  active={section === "planner"}
                  source={source}
                  snapshot={snapshot}
                  onError={setActionError}
                  query={queries.planner ?? ""}
                  onQueryChange={(value) =>
                    setQueries((previous) => ({ ...previous, planner: value }))
                  }
                />
              </section>
              <section
                id="view-settings"
                aria-label={uiText("deviceSettings")}
                hidden={section !== "settings"}
              >
                <LaunchCard
                  source={source}
                  snapshot={snapshot}
                  onError={setActionError}
                />
              </section>
              {section === "remote" && phase === "signedIn" ? (
                <section id="view-remote" aria-label={remoteText("title")}>
                  <RemotePage
                    key={accountId ?? "signedOut"}
                    source={remoteSource}
                    view={snapshot?.remote ?? { sessions: [] }}
                    devices={snapshot?.devices ?? []}
                    ownId={
                      snapshot?.session.phase === "signedIn"
                        ? snapshot.session.device.id
                        : null
                    }
                    onSendFiles={() => {
                      void source
                        .startSend()
                        .catch((error: unknown) =>
                          toast.error(messageFor(toBridgeError(error))),
                        );
                    }}
                  />
                </section>
              ) : null}
              {section === "memos" ? (
                <section id="view-memos" aria-label={memoText("title")}>
                  <Suspense
                    fallback={
                      <LoadingState
                        className="h-64 w-full"
                        label={memoText("loading")}
                      />
                    }
                  >
                    <MemosPage
                      key={accountId ?? "signedOut"}
                      source={memoSource}
                      accountId={accountId}
                      onSignIn={() => navigate("account")}
                    />
                  </Suspense>
                </section>
              ) : null}
              {section === "media" ? (
                <section id="view-media" aria-label={uiText("moviesAndTV")}>
                  <Suspense
                    fallback={
                      <LoadingState
                        className="h-64 w-full"
                        label={uiText("loadingMediaLibrary")}
                      />
                    }
                  >
                    <MediaPage
                      key={accountId ?? "anonymous"}
                      source={mediaSource}
                      accountId={accountId}
                      onSignIn={() => navigate("account")}
                      onLocationChange={setMediaLocation}
                      onBrowseLibrary={() => {
                        void source
                          .openPublicMedia()
                          .catch((error: unknown) =>
                            toast.error(messageFor(toBridgeError(error))),
                          );
                      }}
                    />
                  </Suspense>
                </section>
              ) : null}
              <section
                id="view-account"
                aria-label={uiText("accountCenter")}
                hidden={section !== "account"}
              >
                <div
                  key={accountId ?? "signedOut"}
                  className="flex flex-col gap-4"
                >
                  <SessionCard
                    source={source}
                    snapshot={snapshot}
                    onError={setActionError}
                    onSignOut={signOut}
                    busy={accountPending}
                  />
                  <DevicesCard
                    source={source}
                    snapshot={snapshot}
                    onError={setActionError}
                  />
                </div>
              </section>
              <section
                id="view-about"
                aria-label={uiText("about")}
                hidden={section !== "about"}
              >
                <AboutCard
                  source={source}
                  snapshot={snapshot}
                  onError={setActionError}
                />
              </section>
            </div>
          </ScrollArea>
        </main>
      </SidebarProvider>
    </>
  );
}

/** 版本、更新与官网只在关于页展示，避免每个页面重复占用内容空间。 */
function AboutCard({ source, snapshot, onError }: CardProps) {
  const [busy, setBusy] = useState(false);
  const update = snapshot?.update;
  // 读回的结果由 Rust 经状态推送回来，界面不自行改状态。
  const run = async (action: () => Promise<unknown>) => {
    setBusy(true);
    onError(null);
    try {
      await action();
    } catch (error) {
      onError(toBridgeError(error).code);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Card>
      <CardHeader className="sr-only">
        <CardTitle>{uiText("about")}</CardTitle>
      </CardHeader>
      <CardContent>
        <dl className="flex flex-col gap-4 text-sm">
          <div className="flex flex-col gap-1">
            <dt className="text-muted-foreground">{uiText("version")}</dt>
            <dd className="select-text">
              {formatBuildInfo(__ASSEMBLE_BUILD__)}
            </dd>
          </div>
          <div className="flex flex-col gap-1">
            <dt className="text-muted-foreground">{uiText("website")}</dt>
            <dd className="break-all select-text">
              {__ASSEMBLE_BUILD__.website}
            </dd>
          </div>
          {update ? (
            <div className="flex flex-col gap-1">
              <dt className="text-muted-foreground">{uiText("update")}</dt>
              <dd className="flex flex-wrap items-center justify-between gap-3">
                <span data-testid="update-status">{updateStatus(update)}</span>
                {update.state === "ready" ? (
                  <Button
                    size="sm"
                    disabled={busy}
                    onClick={() => void run(() => source.updateRestart())}
                  >
                    {uiText("updateRestart")}
                  </Button>
                ) : update.state === "unsupported" ? null : (
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={busy || update.state === "checking"}
                    onClick={() => void run(() => source.updateCheck())}
                  >
                    {uiText("updateCheck")}
                  </Button>
                )}
              </dd>
            </div>
          ) : null}
        </dl>
      </CardContent>
    </Card>
  );
}

/** 页头只保留检测、断网与登录后的连接状态。 */
function ServiceStatus({
  service,
  signedIn,
}: {
  service: ServiceView | undefined;
  signedIn: boolean;
}) {
  if (!service || service.reachable === null) {
    return (
      <p className="text-sm text-muted-foreground">
        {uiText("checkingConnection")}
      </p>
    );
  }
  if (!service.reachable) {
    return (
      <p className="text-sm text-destructive">
        {signedIn
          ? uiText("connectionLostReconnecting")
          : uiText("unableToReachTheServer")}
      </p>
    );
  }
  if (signedIn) {
    return (
      <p className="text-sm text-muted-foreground">
        {service.stream === "connected"
          ? uiText("connected")
          : // `off` 只出现在刚登录、事件流还没开始建立的那一瞬间。
            service.stream === "connecting" || service.stream === "off"
            ? uiText("connectingEllipsis")
            : uiText("connectionLostReconnectingEllipsis")}
      </p>
    );
  }
  return null;
}

/** 提示是 Rust 给的一句话；收起来只要告诉它一声。 */
async function dismiss(source: Bridge) {
  try {
    await source.dismissNotice();
  } catch (error: unknown) {
    // 收不起来不影响使用：下一次状态推送自然会换掉它。
    console.warn(messageFor(toBridgeError(error)));
  }
}

function NoticeLine({
  notice,
  onDismiss,
}: {
  notice: Notice;
  onDismiss: () => void;
}) {
  if (notice.kind === "error" && notice.message === LOGIN_TIMEOUT_MESSAGE) {
    return <LoginTimeoutToast onDismiss={onDismiss} />;
  }
  const isError = notice.kind === "error";
  return (
    <Alert
      variant={isError ? "destructive" : "default"}
      role={isError ? "alert" : "status"}
    >
      <AlertDescription>{noticeText(notice)}</AlertDescription>
      <AlertAction>
        <Button type="button" variant="ghost" size="sm" onClick={onDismiss}>
          {uiText("dismiss")}
        </Button>
      </AlertAction>
    </Alert>
  );
}

function LoginTimeoutToast({ onDismiss }: { onDismiss: () => void }) {
  useDesktopTranslation();
  const message = uiText("browserSignInTimedOutPleaseTryAgain");
  const dismissRef = useRef(onDismiss);
  dismissRef.current = onDismiss;

  useEffect(() => {
    toast.error(message, {
      id: "login-timeout",
      duration: Number.POSITIVE_INFINITY,
      onDismiss: () => dismissRef.current(),
    });
  }, [message]);
  useEffect(
    () => () => {
      toast.dismiss("login-timeout");
    },
    [],
  );

  return null;
}

interface CardProps {
  source: Bridge;
  snapshot: Snapshot | null;
  onError: (message: string | null) => void;
}

function SessionCard({
  source,
  snapshot,
  onError,
  onSignOut,
  busy = false,
}: CardProps & { onSignOut?: () => Promise<void>; busy?: boolean }) {
  const [pending, setPending] = useState(false);
  const session = snapshot?.session;
  const unreachable = snapshot?.service.reachable === false;

  async function perform(action: () => Promise<unknown>) {
    setPending(true);
    onError(null);
    try {
      await action();
    } catch (error: unknown) {
      const failure = toBridgeError(error);
      // 回调失败的具体提示由 Rust 的 notice 提供，取消或拒绝时则不提示。
      // 不再叠上一条通用错误，否则超时 toast 之外页面里还会留着同一次失败。
      if (failure.code !== "LOGIN_CALLBACK_FAILED") {
        onError(failure.code);
      }
    } finally {
      setPending(false);
    }
  }

  const action = (() => {
    if (!session || pending || busy) return null;
    switch (session.phase) {
      case "signingIn":
        return (
          <Button
            variant="outline"
            onClick={() => void perform(() => source.cancelSignIn())}
          >
            {uiText("cancel")}
          </Button>
        );
      case "signedIn":
        return (
          <Button
            variant="outline"
            onClick={() =>
              void perform(() => (onSignOut ? onSignOut() : source.signOut()))
            }
          >
            {uiText("signOut")}
          </Button>
        );
      case "signedOut":
        return (
          <Button
            disabled={unreachable}
            onClick={() => void perform(() => source.signIn())}
          >
            {uiText("signInWithBrowser")}
          </Button>
        );
    }
  })();

  return (
    <Card>
      <CardHeader>
        <CardTitle>{uiText("account")}</CardTitle>
        <CardDescription>{describe(session, unreachable)}</CardDescription>
        <CardAction>{action}</CardAction>
      </CardHeader>
      {session?.phase === "signedIn" ? (
        <CardContent className="flex items-center gap-3">
          <Avatar>
            {session.account.avatarUrl ? (
              <AvatarImage
                src={session.account.avatarUrl}
                alt=""
                referrerPolicy="no-referrer"
              />
            ) : null}
            <AvatarFallback>
              {Array.from(session.account.displayName)[0]?.toUpperCase() || "A"}
            </AvatarFallback>
          </Avatar>
          <p className="text-sm text-muted-foreground">
            {uiText("thisDevice")}
            {session.device.name}
          </p>
        </CardContent>
      ) : null}
    </Card>
  );
}

function describe(
  session: Snapshot["session"] | undefined,
  unreachable: boolean,
): string {
  if (!session) return "";
  switch (session.phase) {
    case "signingIn":
      return uiText("theSignInPageIsOpenInYourBrowserReturnHereWhenYouFinish");
    case "signedIn": {
      const { displayName, username, email } = session.account;
      const identity = email ?? username;
      return displayName === identity
        ? identity
        : `${displayName}（${identity}）`;
    }
    case "signedOut":
      return unreachable
        ? uiText("theServerIsUnavailableSignInWhenTheConnectionIsRestored")
        : "";
  }
}

/** 设备名最长多少个字，与 Rust 侧一致。超出就不必发出去。 */
const DEVICE_NAME_MAX = 60;

/** 账户里的设备。只在登录之后出现。 */
function DevicesCard({ source, snapshot, onError }: CardProps) {
  if (snapshot?.session.phase !== "signedIn") return null;
  const session = snapshot.session;

  return (
    <Card>
      <CardHeader>
        <CardTitle>{uiText("devices")}</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col">
        {snapshot.devices.map((device, index) => (
          <Fragment key={device.id}>
            {index > 0 ? <Separator /> : null}
            <DeviceRow
              source={source}
              device={device}
              isThisDevice={device.id === session.device.id}
              onError={onError}
            />
          </Fragment>
        ))}
      </CardContent>
    </Card>
  );
}

function DeviceRow({
  source,
  device,
  isThisDevice,
  onError,
}: {
  source: Bridge;
  device: DeviceView;
  isThisDevice: boolean;
  onError: (message: string | null) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [confirming, setConfirming] = useState(false);
  // 只在编辑时用；点"改名"时从当前名字开始。
  const [draft, setDraft] = useState(device.name);
  const [pending, setPending] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  async function perform(action: () => Promise<unknown>) {
    setPending(true);
    setProblem(null);
    onError(null);
    try {
      await action();
      setEditing(false);
      setConfirming(false);
    } catch (error: unknown) {
      setProblem(toBridgeError(error).code);
    } finally {
      setPending(false);
    }
  }

  function save() {
    const name = draft.trim();
    if (name.length === 0 || name.length > DEVICE_NAME_MAX) {
      setProblem(toBridgeError({ code: "INVALID_INPUT" }).code);
      return;
    }
    if (name === device.name) {
      setEditing(false);
      return;
    }
    void perform(() => source.renameDevice(device.id, name));
  }

  return (
    <div className="py-3">
      <div className="flex flex-wrap items-center gap-3">
        <span
          aria-hidden
          className={
            device.online
              ? "size-2 shrink-0 rounded-full bg-emerald-500"
              : "size-2 shrink-0 rounded-full bg-muted-foreground/40"
          }
        />
        <div className="min-w-0 flex-1">
          {editing ? (
            <Input
              aria-label={uiText("newNameFor", { value0: device.name })}
              value={draft}
              maxLength={DEVICE_NAME_MAX}
              autoFocus
              disabled={pending}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") save();
                if (event.key === "Escape") {
                  setEditing(false);
                  setDraft(device.name);
                }
              }}
            />
          ) : (
            <p className="truncate text-sm font-medium">
              {device.name}
              {isThisDevice ? (
                <Badge className="ml-2" variant="secondary">
                  {uiText("thisDevice2")}
                </Badge>
              ) : null}
            </p>
          )}
          <p className="text-xs text-muted-foreground">
            {platformName(device.platform)} ·{" "}
            {device.online ? uiText("online") : uiText("offline")}
          </p>
        </div>
        {isThisDevice ? null : (
          <div className="flex max-w-full shrink-0 flex-wrap gap-2">
            {confirming ? (
              <>
                <Button
                  size="default"
                  variant="destructive"
                  disabled={pending}
                  onClick={() =>
                    void perform(() => source.removeDevice(device.id))
                  }
                >
                  {uiText("confirmRemoval")}
                </Button>
                <Button
                  size="default"
                  variant="outline"
                  disabled={pending}
                  onClick={() => setConfirming(false)}
                >
                  {uiText("cancel")}
                </Button>
              </>
            ) : editing ? (
              <>
                <Button size="default" disabled={pending} onClick={save}>
                  {uiText("save")}
                </Button>
                <Button
                  size="default"
                  variant="outline"
                  disabled={pending}
                  onClick={() => {
                    setEditing(false);
                    setDraft(device.name);
                  }}
                >
                  {uiText("cancel")}
                </Button>
              </>
            ) : (
              <>
                <Button
                  size="default"
                  variant="outline"
                  onClick={() => {
                    setDraft(device.name);
                    setEditing(true);
                  }}
                >
                  {uiText("rename")}
                </Button>
                <Button
                  size="default"
                  variant="outline"
                  onClick={() => setConfirming(true)}
                >
                  {uiText("remove")}
                </Button>
              </>
            )}
          </div>
        )}
      </div>
      {problem ? (
        <FieldError className="mt-1">
          {messageFor(toBridgeError({ code: problem }))}
        </FieldError>
      ) : null}
    </div>
  );
}

/** 还没到头：可以取消。 */
const CANCELLABLE: ReadonlySet<TransferView["phase"]> = new Set([
  "queued",
  "inProgress",
  "waiting",
]);

/** 文件传输。收到的自动存进接收位置；发出去的从这里发起，也在这里看得到结果。 */
function TransfersCard({
  source,
  snapshot,
  onError,
  query,
  onQueryChange,
}: CardProps & { query: string; onQueryChange: (query: string) => void }) {
  const [pending, setPending] = useState(false);

  // 选文件时系统选择器一直开着，命令要到它关掉才返回。
  async function startSend() {
    setPending(true);
    onError(null);
    try {
      await source.startSend();
    } catch (error: unknown) {
      onError(toBridgeError(error).code);
    } finally {
      setPending(false);
    }
  }

  // 没登录就没有传输记录，这一块不出现。
  if (snapshot == null || snapshot.session.phase !== "signedIn") return null;

  return (
    <Card>
      <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-3">
        <CardTitle className="sr-only">{uiText("transfers")}</CardTitle>
        <Input
          className="w-full sm:w-64"
          aria-label={uiText("searchFileTransfers")}
          placeholder={uiText("searchEllipsis")}
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
        />
        <CardAction className="self-center">
          <Button
            size="default"
            variant="outline"
            disabled={pending}
            onClick={() => void startSend()}
          >
            {uiText("sendFiles")}
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col">
        {snapshot.transfers.length === 0 ? (
          <Empty>
            <EmptyHeader>
              <EmptyTitle>{uiText("noTransfersYet")}</EmptyTitle>
            </EmptyHeader>
          </Empty>
        ) : snapshot.transfers.filter((item) =>
            `${item.name} ${item.peer} ${transferStatus(item)}`
              .toLocaleLowerCase()
              .includes(query.trim().toLocaleLowerCase()),
          ).length === 0 ? (
          <Empty>
            <EmptyHeader>
              <EmptyTitle>{uiText("noMatchingTransfers")}</EmptyTitle>
            </EmptyHeader>
          </Empty>
        ) : (
          snapshot.transfers
            .filter((transfer) =>
              `${transfer.name} ${transfer.peer} ${transferStatus(transfer)}`
                .toLocaleLowerCase()
                .includes(query.trim().toLocaleLowerCase()),
            )
            .map((transfer, index) => (
              <Fragment key={transfer.id}>
                {index > 0 ? <Separator /> : null}
                <TransferRow
                  source={source}
                  transfer={transfer}
                  onError={onError}
                />
              </Fragment>
            ))
        )}
      </CardContent>
    </Card>
  );
}

function TransferRow({
  source,
  transfer,
  onError,
  showPeer = true,
}: {
  source: Bridge;
  transfer: TransferView;
  onError: (message: string | null) => void;
  showPeer?: boolean;
}) {
  const [pending, setPending] = useState(false);
  const failed = transfer.phase === "failed";
  const receiving = transfer.direction === "in";
  const cloudTransfer = transfer.target === "drive";
  const status = transferStatus(transfer);
  const savedHere = isSavedHere(transfer);

  // 一条记录上的几个按钮做的是同一类事：交出标识，等 Rust 办完。
  async function act(what: () => Promise<unknown>) {
    setPending(true);
    onError(null);
    try {
      await what();
    } catch (error: unknown) {
      onError(toBridgeError(error).code);
    } finally {
      setPending(false);
    }
  }

  return (
    <article
      id={`transfer-${transfer.id}`}
      tabIndex={-1}
      aria-label={transfer.name}
      className="py-3"
    >
      <div className="flex flex-wrap items-center gap-3">
        {showPeer ? (
          <span
            aria-hidden
            className="text-muted-foreground"
            data-direction={transfer.direction}
          >
            {receiving ? (
              <ArrowDownToLine className="size-4" />
            ) : (
              <ArrowUpToLine className="size-4" />
            )}
          </span>
        ) : null}
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium" title={transfer.name}>
            {transfer.name}
          </p>
          <p
            role={cloudTransfer && failed ? "alert" : undefined}
            className={
              cloudTransfer && failed
                ? "text-xs text-destructive"
                : "text-xs text-muted-foreground"
            }
          >
            {cloudTransfer
              ? `${status} · `
              : showPeer
                ? `${receiving ? uiText("from") : uiText("to")} ${transfer.peer} · `
                : ""}
            {formatSize(transfer.size)}
          </p>
          {transfer.savedAs ? (
            <p className="text-xs text-muted-foreground">
              {uiText("savedAsFilename", { name: transfer.savedAs })}
            </p>
          ) : null}
        </div>
        <div className="ms-auto flex max-w-full shrink-0 flex-wrap items-center gap-2">
          {savedHere ? (
            // 只在文件夹里选中它，不打开：收到的东西要不要打开由用户自己决定。
            <Button
              size="default"
              variant="outline"
              disabled={pending}
              onClick={() => void act(() => source.revealReceived(transfer.id))}
            >
              {uiText("showInFolder")}
            </Button>
          ) : null}
          {transfer.retryable ? (
            <Button
              size="default"
              variant="outline"
              disabled={pending}
              onClick={() => void act(() => source.retryTransfer(transfer.id))}
            >
              {uiText("retry")}
            </Button>
          ) : null}
          {CANCELLABLE.has(transfer.phase) ? (
            <Button
              size="default"
              variant="outline"
              disabled={pending}
              onClick={() => void act(() => source.cancelTransfer(transfer.id))}
            >
              {uiText("cancel")}
            </Button>
          ) : null}
        </div>
      </div>
      {transfer.phase === "inProgress" || transfer.phase === "queued" ? (
        <Progress
          aria-label={uiText("transferProgressFor", { value0: transfer.name })}
          className="mt-2"
          value={
            transfer.phase === "queued"
              ? 0
              : Math.floor(transfer.progress * 100)
          }
          getValueLabel={() => status}
        />
      ) : null}
      {!cloudTransfer && !savedHere ? (
        <p
          role={failed ? "alert" : undefined}
          className={
            failed
              ? "mt-1 text-xs text-destructive"
              : "mt-1 text-xs text-muted-foreground"
          }
        >
          {status}
        </p>
      ) : null}
    </article>
  );
}

/** 云盘：账户在服务端的文件。从这台电脑存进去的，别的设备也取得到。 */
function DriveCard({
  source,
  snapshot,
  onError,
  query,
  onQueryChange,
}: CardProps & { query: string; onQueryChange: (query: string) => void }) {
  const [pending, setPending] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const uploadStarting = useRef(false);
  const previousUploads = useRef(new Set<string>());
  const [completedUploads, setCompletedUploads] = useState<string[]>([]);
  const [confirming, setConfirming] = useState<string | null>(null);
  // 名单由 Rust 在连上服务时去拿；万一没拿到（比如刚登录时断了一下），这里补要一次。
  const requested = useRef<string | null>(null);
  const signedInAs =
    snapshot?.session.phase === "signedIn" ? snapshot.session.device.id : null;
  const drive = snapshot?.drive ?? null;

  useEffect(() => {
    const uploads =
      snapshot?.transfers.filter(
        (item) => item.target === "drive" && item.direction === "out",
      ) ?? [];
    const completed = uploads.filter(
      (item) =>
        item.phase === "complete" && previousUploads.current.has(item.id),
    );
    if (completed.length)
      setCompletedUploads(completed.map((item) => item.name));
    previousUploads.current = new Set(
      uploads
        .filter(
          (item) => item.phase === "queued" || item.phase === "inProgress",
        )
        .map((item) => item.id),
    );
  }, [snapshot?.transfers]);

  useEffect(() => {
    if (!signedInAs || drive || requested.current === signedInAs) return;
    requested.current = signedInAs;
    source.driveRefresh().catch((error: unknown) => {
      onError(toBridgeError(error).code);
    });
  }, [signedInAs, drive, source, onError]);

  if (snapshot == null || snapshot.session.phase !== "signedIn") return null;

  async function act(what: () => Promise<unknown>) {
    setPending(true);
    onError(null);
    try {
      await what();
    } catch (error: unknown) {
      onError(toBridgeError(error).code);
    } finally {
      setPending(false);
      setConfirming(null);
    }
  }

  async function upload() {
    if (uploadStarting.current) return;
    uploadStarting.current = true;
    setPreparing(true);
    setCompletedUploads([]);
    try {
      await act(() => source.driveUpload());
    } finally {
      uploadStarting.current = false;
      setPreparing(false);
    }
  }

  // 上传任务不受文件搜索过滤，避免新选的文件在筛选条件下看不见进度。
  const uploads = snapshot.transfers.filter(
    (item) =>
      item.target === "drive" &&
      item.direction === "out" &&
      (item.phase === "queued" ||
        item.phase === "inProgress" ||
        item.phase === "failed"),
  );
  const files =
    drive?.items.filter((file) =>
      file.name.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()),
    ) ?? [];

  return (
    <Card>
      <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-3">
        <CardTitle className="sr-only">{uiText("cloudDrive")}</CardTitle>
        <CardDescription
          className="shrink-0 whitespace-nowrap"
          title={uiText("usedSpaceTotalCapacity")}
        >
          {drive
            ? `${formatSpace(drive.usedBytes)} / ${formatSpace(drive.limitBytes)}`
            : uiText("loadingCloudDriveEllipsis")}
        </CardDescription>
        <CardAction className="flex min-w-0 flex-1 items-center gap-2 self-center sm:max-w-xs">
          <Input
            className="min-w-0 flex-1"
            aria-label={uiText("searchCloudDrive")}
            placeholder={uiText("searchEllipsis")}
            value={query}
            onChange={(event) => onQueryChange(event.target.value)}
          />
          <Button
            className="shrink-0"
            size="default"
            variant="outline"
            disabled={pending}
            onClick={() => void upload()}
          >
            {preparing ? uiText("preparing") : uiText("upload")}
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col">
        <p role="status" className="sr-only">
          {completedUploads
            .map((name) => uiText("uploaded", { value0: name }))
            .join("; ")}
        </p>
        {uploads.map((transfer, index) => (
          <Fragment key={transfer.id}>
            {index > 0 ? <Separator /> : null}
            <TransferRow
              source={source}
              transfer={transfer}
              onError={onError}
              showPeer={false}
            />
          </Fragment>
        ))}
        {drive ? (
          drive.items.length === 0 && uploads.length === 0 ? (
            <Empty>
              <EmptyHeader>
                <EmptyTitle>{uiText("noFilesInCloudDriveYet")}</EmptyTitle>
              </EmptyHeader>
            </Empty>
          ) : drive.items.length > 0 && files.length === 0 ? (
            <Empty>
              <EmptyHeader>
                <EmptyTitle>{uiText("noMatchingFiles")}</EmptyTitle>
              </EmptyHeader>
            </Empty>
          ) : (
            files.map((file, index) => (
              <Fragment key={file.id}>
                {index > 0 || uploads.length > 0 ? <Separator /> : null}
                <DriveRow
                  source={source}
                  file={file}
                  confirming={confirming === file.id}
                  pending={pending}
                  onConfirm={() => setConfirming(file.id)}
                  onCancel={() => setConfirming(null)}
                  act={act}
                />
              </Fragment>
            ))
          )
        ) : null}
      </CardContent>
    </Card>
  );
}

function DriveRow({
  source,
  file,
  confirming,
  pending,
  onConfirm,
  onCancel,
  act,
}: {
  source: Bridge;
  file: DriveFileView;
  confirming: boolean;
  pending: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  act: (what: () => Promise<unknown>) => Promise<void>;
}) {
  return (
    <div className="flex flex-wrap items-start gap-3 py-3">
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium" title={file.name}>
          {file.name}
        </p>
        <p className="text-xs text-muted-foreground">
          {formatSize(file.size)} · {formatStoredDate(file.createdAt)}
        </p>
      </div>
      <div className="flex max-w-full shrink-0 flex-wrap gap-2">
        {confirming ? (
          <>
            <Button
              size="default"
              variant="destructive"
              disabled={pending}
              onClick={() => void act(() => source.driveDelete(file.id))}
            >
              {uiText("confirmDeletion")}
            </Button>
            <Button
              size="default"
              variant="outline"
              disabled={pending}
              onClick={onCancel}
            >
              {uiText("cancel")}
            </Button>
          </>
        ) : (
          <>
            <Button
              size="default"
              variant="outline"
              disabled={pending}
              onClick={() => void act(() => source.driveDownload(file.id))}
            >
              {uiText("download")}
            </Button>
            <DriveRenameAction source={source} file={file} disabled={pending} />
            <Button
              size="default"
              variant="outline"
              disabled={pending}
              onClick={onConfirm}
            >
              {uiText("delete")}
            </Button>
          </>
        )}
      </div>
    </div>
  );
}

const launchHints = (): Record<LaunchState, string | null> => ({
  enabled: null,
  disabled: null,
  unsupported: uiText("launchAtSignInIsUnavailableInDevelopmentBuilds"),
  unknown: uiText("waitingForSystemApproval"),
});

/** 默认的接收位置。它是一个固定的系统位置，不是哪个具体文件夹的路径。 */

function LaunchCard({ source, snapshot, onError }: CardProps) {
  const [pending, setPending] = useState(false);
  const state: LaunchState = snapshot?.launchAtLogin ?? "unknown";
  const hint = launchHints()[state];
  const customFolder = snapshot?.receive.customFolder ?? null;
  const receivePath = snapshot?.receive.displayPath ?? null;
  // 只有 Windows 上的打包运行才有右键菜单这一项。
  const sendMenu = snapshot?.sendMenu ?? "unsupported";

  // 开关类设置都是一回事：把要改成什么交给 Rust，再由它把系统的实际状态推回来。
  async function flip(action: () => Promise<unknown>) {
    setPending(true);
    onError(null);
    try {
      await action();
    } catch (error: unknown) {
      onError(toBridgeError(error).code);
    } finally {
      setPending(false);
    }
  }

  // 选文件夹时系统选择器一直开着，命令要到用户关掉它才返回。
  async function locate(action: () => Promise<unknown>) {
    setPending(true);
    onError(null);
    try {
      await action();
    } catch (error: unknown) {
      onError(toBridgeError(error).code);
    } finally {
      setPending(false);
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="sr-only">{uiText("thisDevice2")}</CardTitle>
        <CardDescription>{uiText("launchAtSignIn")}</CardDescription>
        <CardAction>
          <Switch
            aria-label={uiText("launchAtSignIn")}
            checked={state === "enabled"}
            disabled={pending || state === "unsupported"}
            onCheckedChange={(checked) =>
              void flip(() => source.setLaunchAtLogin(checked))
            }
          />
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {hint ? <p className="text-sm text-muted-foreground">{hint}</p> : null}
        <div className="flex flex-wrap items-center justify-between gap-3 pt-3">
          <div className="min-w-0">
            <p className="text-sm font-medium">{uiText("downloadLocation")}</p>
            <p
              className={cn(
                "text-sm text-muted-foreground",
                receivePath === null ? "truncate" : "break-all select-text",
              )}
            >
              {receivePath ??
                (customFolder === null
                  ? uiText("downloadsAssemble")
                  : uiText("folder", { value0: customFolder }))}
            </p>
          </div>
          <div className="flex max-w-full shrink-0 flex-wrap gap-2">
            <Button
              size="default"
              variant="outline"
              disabled={pending}
              onClick={() => void locate(() => source.chooseReceiveFolder())}
            >
              {uiText("change")}
            </Button>
            {customFolder !== null ? (
              <Button
                size="default"
                variant="outline"
                disabled={pending}
                onClick={() => void locate(() => source.resetReceiveFolder())}
              >
                {uiText("restoreDefault")}
              </Button>
            ) : null}
          </div>
        </div>
        {snapshot?.downloads ? (
          <AssistantDownloadsSetting
            source={source}
            enabled={snapshot.downloads.assistantAllowed}
          />
        ) : null}
        {/* 提醒有没有用，要看登录没登录：没登录就收不到，这一项也不必摆出来。 */}
        {snapshot?.session.phase === "signedIn" ? (
          <div className="flex flex-wrap items-center justify-between gap-3 pt-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">
                {uiText("reminderNotifications")}
              </p>
              <p className="text-sm text-muted-foreground">
                {snapshot.plannerMuted
                  ? uiText(
                      "remindersStayInYourScheduleWithoutNotifyingThisDevice",
                    )
                  : uiText(
                      "showSystemNotificationsOnThisDeviceWhenSchedulesAndRemindersAreDue",
                    )}
              </p>
            </div>
            <Switch
              aria-label={uiText("notifyWhenDue")}
              checked={!snapshot.plannerMuted}
              disabled={pending}
              onCheckedChange={(checked) =>
                void flip(() => source.setPlannerMuted(!checked))
              }
            />
          </div>
        ) : null}
        {sendMenu === "unsupported" ? null : (
          <div className="flex flex-col gap-3">
            <Separator />
            <div className="flex items-center justify-between gap-3">
              <div className="min-w-0">
                <p className="text-sm font-medium">
                  {uiText("fileContextMenu")}
                </p>
                <p className="text-sm text-muted-foreground">
                  {sendMenu === "enabled"
                    ? uiText("rightClickAFileAndChooseSendWithAssemble")
                    : uiText("sendWithAssembleIsDisabledInTheFileContextMenu")}
                </p>
              </div>
              <Switch
                aria-label={uiText("sendWithAssembleInTheFileContextMenu")}
                checked={sendMenu === "enabled"}
                disabled={pending}
                onCheckedChange={(checked) =>
                  void flip(() => source.setSendMenu(checked))
                }
              />
            </div>
          </div>
        )}
      </CardContent>
    </Card>
  );
}
