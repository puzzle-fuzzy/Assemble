// 应用主界面。
//
// 现在有六类真实内容：账户与连接、账户里的设备、文件传输、日程与提醒、云盘、本机设置。

import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@assemble/ui/breadcrumb";
import { ArrowDownToLine, ArrowUpToLine, UserRound } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import {
  DesktopMobileMenu,
  DesktopSidebar,
  DESKTOP_MODULES as SECTIONS,
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
import { SidebarProvider } from "@/components/ui/sidebar";
import { Toaster } from "@/components/ui/sonner";
import { Switch } from "@/components/ui/switch";
import { useSnapshot } from "@/hooks/use-snapshot";
import { type Bridge, bridge, toBridgeError } from "@/lib/bridge";
import { formatBuildInfo } from "@/lib/build-info";
import { messageFor } from "@/lib/messages";
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
  Platform,
  ServiceView,
  Snapshot,
  TransferView,
} from "@/lib/types";
import { cn } from "@/lib/utils";
import { PlannerCard } from "@/PlannerCard";

export interface AppProps {
  /** 默认使用真实的桥接；测试时注入替身。 */
  bridge?: Bridge;
}

const LOGIN_TIMEOUT_MESSAGE = "等待浏览器登录超时，请重试。";

export function App({ bridge: source = bridge }: AppProps) {
  const { snapshot, loadError } = useSnapshot(source);
  const [actionError, setActionError] = useState<string | null>(null);
  const [section, setSection] = useState<Section>("transfers");
  const [queries, setQueries] = useState<Partial<Record<Section, string>>>({});
  const contentHeading = useRef<HTMLHeadingElement>(null);
  const phase = snapshot?.session.phase;
  const accountId =
    snapshot?.session.phase === "signedIn" ? snapshot.session.account.id : null;
  const previousAccount = useRef(accountId);
  const previousSection = useRef(section);
  const compose = snapshot?.planner?.compose ?? null;
  const focused = snapshot?.planner?.focus ?? null;

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

  const query = queries[section] ?? "";
  const sectionLabel = SECTIONS.find((item) => item.id === section)?.label;
  useEffect(() => {
    document.title = `${sectionLabel} · Assemble`;
    if (previousSection.current !== section) contentHeading.current?.focus();
    previousSection.current = section;
  }, [section, sectionLabel]);

  function navigate(next: Section) {
    setSection(next);
    setActionError(null);
    // Rust 确认清除入口后，同一条通知或同类托盘操作才可再次触发定位。
    if (next !== "planner") {
      if (compose !== null)
        void source
          .plannerCompose(null)
          .catch((error: unknown) =>
            setActionError(messageFor(toBridgeError(error))),
          );
      if (focused !== null)
        void source
          .plannerFocus(null)
          .catch((error: unknown) =>
            setActionError(messageFor(toBridgeError(error))),
          );
    }
  }

  return (
    <>
      <Toaster closeButton containerAriaLabel="登录提示" />
      <SidebarProvider className="items-start">
        <DesktopSidebar section={section} onNavigate={navigate} />
        <main className="flex h-svh min-w-0 flex-1 flex-col overflow-hidden">
          <header className="flex h-16 shrink-0 items-center justify-between gap-2 transition-[width,height] ease-linear group-has-data-[collapsible=icon]/sidebar-wrapper:h-12">
            <div className="flex min-w-0 items-center gap-2 px-4">
              <DesktopMobileMenu
                section={section}
                onNavigate={navigate}
                onNavigateFocus={() => contentHeading.current?.focus()}
              />
              <Breadcrumb>
                <BreadcrumbList>
                  <BreadcrumbItem className="hidden md:block">
                    <BreadcrumbLink
                      href="#transfers"
                      onClick={(event) => {
                        event.preventDefault();
                        navigate("transfers");
                      }}
                    >
                      Assemble
                    </BreadcrumbLink>
                  </BreadcrumbItem>
                  <BreadcrumbSeparator className="hidden md:block" />
                  <BreadcrumbItem>
                    <BreadcrumbPage>{sectionLabel}</BreadcrumbPage>
                  </BreadcrumbItem>
                </BreadcrumbList>
              </Breadcrumb>
            </div>
            <div className="min-w-0 shrink-0 px-4">
              <ServiceStatus
                service={snapshot?.service}
                signedIn={phase === "signedIn"}
              />
            </div>
          </header>
          <ScrollArea className="min-h-0 flex-1">
            <div className="@container mx-auto flex w-full min-w-0 flex-col gap-4 p-4 pt-0">
              <div className="flex flex-wrap items-center justify-between gap-3">
                <h1
                  ref={contentHeading}
                  tabIndex={-1}
                  className="text-xl font-semibold tracking-tight"
                >
                  {sectionLabel}
                </h1>
                {phase === "signedIn" &&
                section !== "settings" &&
                section !== "account" ? (
                  <Input
                    className="w-full sm:w-64"
                    aria-label={`搜索${sectionLabel}`}
                    placeholder="搜索…"
                    value={query}
                    onChange={(event) => {
                      const value = event.target.value;
                      setQueries((previous) => ({
                        ...previous,
                        [section]: value,
                      }));
                    }}
                  />
                ) : null}
              </div>
              {loadError ? (
                <p role="alert" className="text-sm text-destructive">
                  {loadError}
                </p>
              ) : null}
              {snapshot?.notice ? (
                <NoticeLine
                  notice={snapshot.notice}
                  onDismiss={() => void dismiss(source)}
                />
              ) : null}
              {actionError ? (
                <p role="alert" className="text-sm text-destructive">
                  {actionError}
                </p>
              ) : null}
              {phase !== "signedIn" &&
              section !== "settings" &&
              section !== "account" ? (
                <Empty className="min-h-64">
                  <EmptyHeader>
                    <EmptyMedia variant="icon">
                      <UserRound aria-hidden="true" />
                    </EmptyMedia>
                    <EmptyTitle>
                      {snapshot ? "登录 Assemble" : "正在读取本机状态…"}
                    </EmptyTitle>
                    <EmptyDescription>
                      通过左侧菜单的账户中心，在系统浏览器里登录。
                    </EmptyDescription>
                  </EmptyHeader>
                  <EmptyContent>
                    <Button
                      variant="outline"
                      onClick={() => navigate("account")}
                    >
                      打开账户中心
                    </Button>
                  </EmptyContent>
                </Empty>
              ) : null}
              {/* 只隐藏非当前页面，同一账户的草稿与进行中的操作继续保留。 */}
              <section
                id="view-transfers"
                aria-label="文件传输"
                hidden={section !== "transfers"}
              >
                <TransfersCard
                  key={accountId ?? "signedOut"}
                  source={source}
                  snapshot={snapshot}
                  onError={setActionError}
                  query={queries.transfers ?? ""}
                />
              </section>
              <section
                id="view-drive"
                aria-label="云盘"
                hidden={section !== "drive"}
              >
                <DriveCard
                  key={accountId ?? "signedOut"}
                  source={source}
                  snapshot={snapshot}
                  onError={setActionError}
                  query={queries.drive ?? ""}
                />
              </section>
              <section
                id="view-planner"
                aria-label="日程与提醒"
                hidden={section !== "planner"}
              >
                <PlannerCard
                  key={accountId ?? "signedOut"}
                  active={section === "planner"}
                  source={source}
                  snapshot={snapshot}
                  onError={setActionError}
                  query={queries.planner ?? ""}
                />
              </section>
              <section
                id="view-settings"
                aria-label="本机设置"
                hidden={section !== "settings"}
              >
                <LaunchCard
                  source={source}
                  snapshot={snapshot}
                  onError={setActionError}
                />
              </section>
              <section
                id="view-account"
                aria-label="账户中心"
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
                  />
                  <DevicesCard
                    source={source}
                    snapshot={snapshot}
                    onError={setActionError}
                  />
                </div>
              </section>
            </div>
          </ScrollArea>
          <footer className="shrink-0 border-t px-4 py-3 text-center text-xs text-muted-foreground">
            <p>{formatBuildInfo(__ASSEMBLE_BUILD__)}</p>
            <p className="mt-1 break-all select-text">
              官网：{__ASSEMBLE_BUILD__.website}
            </p>
          </footer>
        </main>
      </SidebarProvider>
    </>
  );
}

/** 官网地址由页脚展示；这里仅保留检测、断网与登录后的连接状态。 */
function ServiceStatus({
  service,
  signedIn,
}: {
  service: ServiceView | undefined;
  signedIn: boolean;
}) {
  if (!service || service.reachable === null) {
    return <p className="text-sm text-muted-foreground">正在检测连接</p>;
  }
  if (!service.reachable) {
    return (
      <p className="text-sm text-destructive">
        {signedIn ? "连接已断开，正在重连" : "暂时无法连接服务器"}
      </p>
    );
  }
  if (signedIn) {
    return (
      <p className="text-sm text-muted-foreground">
        {service.stream === "connected"
          ? "已连接"
          : // `off` 只出现在刚登录、事件流还没开始建立的那一瞬间。
            service.stream === "connecting" || service.stream === "off"
            ? "正在连接…"
            : "连接已断开，正在重连…"}
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
    <div
      role={isError ? "alert" : "status"}
      className={
        isError
          ? "flex items-start justify-between gap-3 text-sm text-destructive"
          : "flex items-start justify-between gap-3 text-sm text-muted-foreground"
      }
    >
      <p>{notice.message}</p>
      <button
        type="button"
        className="shrink-0 text-sm text-muted-foreground underline-offset-2 hover:underline"
        onClick={onDismiss}
      >
        知道了
      </button>
    </div>
  );
}

function LoginTimeoutToast({ onDismiss }: { onDismiss: () => void }) {
  const dismissRef = useRef(onDismiss);
  dismissRef.current = onDismiss;

  useEffect(() => {
    const id = toast.error(LOGIN_TIMEOUT_MESSAGE, {
      id: "login-timeout",
      duration: Number.POSITIVE_INFINITY,
      onDismiss: () => dismissRef.current(),
    });
    return () => {
      toast.dismiss(id);
    };
  }, []);

  return null;
}

interface CardProps {
  source: Bridge;
  snapshot: Snapshot | null;
  onError: (message: string | null) => void;
}

function SessionCard({ source, snapshot, onError }: CardProps) {
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
        onError(messageFor(failure));
      }
    } finally {
      setPending(false);
    }
  }

  const action = (() => {
    if (!session || pending) return null;
    switch (session.phase) {
      case "signingIn":
        return (
          <Button
            variant="outline"
            onClick={() => void perform(() => source.cancelSignIn())}
          >
            取消
          </Button>
        );
      case "signedIn":
        return (
          <Button
            variant="outline"
            onClick={() => void perform(() => source.signOut())}
          >
            退出登录
          </Button>
        );
      case "signedOut":
        return (
          <Button
            disabled={unreachable}
            onClick={() => void perform(() => source.signIn())}
          >
            使用浏览器登录
          </Button>
        );
    }
  })();

  return (
    <Card>
      <CardHeader>
        <CardTitle>账户</CardTitle>
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
            这台设备：{session.device.name}
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
      return "已在浏览器打开登录页，完成后会自动返回。";
    case "signedIn": {
      const { displayName, username, email } = session.account;
      const identity = email ?? username;
      return displayName === identity
        ? identity
        : `${displayName}（${identity}）`;
    }
    case "signedOut":
      return unreachable ? "服务暂不可达，恢复后即可登录。" : "";
  }
}

/** 平台的显示名。 */
const PLATFORM_NAMES: Record<Platform, string> = {
  windows: "Windows",
  macos: "Mac",
  linux: "Linux",
};

/** 设备名最长多少个字，与 Rust 侧一致。超出就不必发出去。 */
const DEVICE_NAME_MAX = 60;

/** 账户里的设备。只在登录之后出现。 */
function DevicesCard({ source, snapshot, onError }: CardProps) {
  if (snapshot?.session.phase !== "signedIn") return null;
  const session = snapshot.session;

  return (
    <Card>
      <CardHeader>
        <CardTitle>设备</CardTitle>
        <CardDescription>
          登录了这个账户的全部电脑。在线的可以直接收文件。
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-2">
        {snapshot.devices.map((device) => (
          <DeviceRow
            key={device.id}
            source={source}
            device={device}
            isThisDevice={device.id === session.device.id}
            onError={onError}
          />
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
      setProblem(messageFor(toBridgeError(error)));
    } finally {
      setPending(false);
    }
  }

  function save() {
    const name = draft.trim();
    if (name.length === 0 || name.length > DEVICE_NAME_MAX) {
      setProblem(messageFor(toBridgeError({ code: "INVALID_INPUT" })));
      return;
    }
    if (name === device.name) {
      setEditing(false);
      return;
    }
    void perform(() => source.renameDevice(device.id, name));
  }

  return (
    <div className="rounded-lg border border-border px-3 py-2">
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
              aria-label={`给「${device.name}」起的新名字`}
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
                  本机
                </Badge>
              ) : null}
            </p>
          )}
          <p className="text-xs text-muted-foreground">
            {PLATFORM_NAMES[device.platform]} ·{" "}
            {device.online ? "在线" : "离线"}
          </p>
        </div>
        {isThisDevice ? null : (
          <div className="flex shrink-0 flex-wrap gap-2">
            {confirming ? (
              <>
                <Button
                  size="sm"
                  variant="destructive"
                  disabled={pending}
                  onClick={() =>
                    void perform(() => source.removeDevice(device.id))
                  }
                >
                  确认移除
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={pending}
                  onClick={() => setConfirming(false)}
                >
                  取消
                </Button>
              </>
            ) : editing ? (
              <>
                <Button size="sm" disabled={pending} onClick={save}>
                  保存
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={pending}
                  onClick={() => {
                    setEditing(false);
                    setDraft(device.name);
                  }}
                >
                  取消
                </Button>
              </>
            ) : (
              <>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => {
                    setDraft(device.name);
                    setEditing(true);
                  }}
                >
                  改名
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => setConfirming(true)}
                >
                  移除
                </Button>
              </>
            )}
          </div>
        )}
      </div>
      {problem ? (
        <p role="alert" className="mt-1 text-xs text-destructive">
          {problem}
        </p>
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
}: CardProps & { query: string }) {
  const [pending, setPending] = useState(false);

  // 选文件时系统选择器一直开着，命令要到它关掉才返回。
  async function startSend() {
    setPending(true);
    onError(null);
    try {
      await source.startSend();
    } catch (error: unknown) {
      onError(messageFor(toBridgeError(error)));
    } finally {
      setPending(false);
    }
  }

  // 没登录就没有传输记录，这一块不出现。
  if (snapshot == null || snapshot.session.phase !== "signedIn") return null;

  return (
    <Card>
      <CardHeader>
        <CardTitle>传输</CardTitle>
        <CardDescription>
          别的设备发来的文件会自动存到接收位置；挑好文件，选一台设备就能发出去。
        </CardDescription>
        <CardAction>
          <Button
            size="sm"
            variant="outline"
            disabled={pending}
            onClick={() => void startSend()}
          >
            发送文件…
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-2">
        {snapshot.transfers.length === 0 ? (
          <Empty>
            <EmptyHeader>
              <EmptyTitle>还没有收发过文件。</EmptyTitle>
              <EmptyDescription>
                发送文件，或等待其他设备发来文件。
              </EmptyDescription>
            </EmptyHeader>
          </Empty>
        ) : snapshot.transfers.filter((item) =>
            `${item.name} ${item.peer} ${transferStatus(item)}`
              .toLocaleLowerCase()
              .includes(query.trim().toLocaleLowerCase()),
          ).length === 0 ? (
          <Empty>
            <EmptyHeader>
              <EmptyTitle>没有匹配的传输</EmptyTitle>
              <EmptyDescription>换个关键词试试。</EmptyDescription>
            </EmptyHeader>
          </Empty>
        ) : (
          snapshot.transfers
            .filter((transfer) =>
              `${transfer.name} ${transfer.peer} ${transferStatus(transfer)}`
                .toLocaleLowerCase()
                .includes(query.trim().toLocaleLowerCase()),
            )
            .map((transfer) => (
              <TransferRow
                key={transfer.id}
                source={source}
                transfer={transfer}
                onError={onError}
              />
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
}: {
  source: Bridge;
  transfer: TransferView;
  onError: (message: string | null) => void;
}) {
  const [pending, setPending] = useState(false);
  const failed = transfer.phase === "failed";
  const receiving = transfer.direction === "in";

  // 一条记录上的几个按钮做的是同一类事：交出标识，等 Rust 办完。
  async function act(what: () => Promise<unknown>) {
    setPending(true);
    onError(null);
    try {
      await what();
    } catch (error: unknown) {
      onError(messageFor(toBridgeError(error)));
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="rounded-lg border border-border px-3 py-2">
      <div className="flex flex-wrap items-start gap-3">
        <span
          aria-hidden
          className="mt-0.5 text-muted-foreground"
          data-direction={transfer.direction}
        >
          {receiving ? (
            <ArrowDownToLine className="size-4" />
          ) : (
            <ArrowUpToLine className="size-4" />
          )}
        </span>
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium" title={transfer.name}>
            {transfer.name}
          </p>
          <p className="text-xs text-muted-foreground">
            {receiving ? "来自" : "发给"} {transfer.peer} ·{" "}
            {formatSize(transfer.size)}
          </p>
          {transfer.savedAs ? (
            <p className="text-xs text-muted-foreground">
              已存为「{transfer.savedAs}」
            </p>
          ) : null}
        </div>
        <div className="flex shrink-0 flex-wrap gap-2">
          {isSavedHere(transfer) ? (
            // 只在文件夹里选中它，不打开：收到的东西要不要打开由用户自己决定。
            <Button
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={() => void act(() => source.revealReceived(transfer.id))}
            >
              在文件夹中显示
            </Button>
          ) : null}
          {transfer.retryable ? (
            <Button
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={() => void act(() => source.retryTransfer(transfer.id))}
            >
              重发
            </Button>
          ) : null}
          {CANCELLABLE.has(transfer.phase) ? (
            <Button
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={() => void act(() => source.cancelTransfer(transfer.id))}
            >
              取消
            </Button>
          ) : null}
        </div>
      </div>
      {transfer.phase === "inProgress" ? (
        <Progress
          aria-hidden
          className="mt-2"
          value={transfer.progress * 100}
        />
      ) : null}
      <p
        role={failed ? "alert" : undefined}
        className={
          failed
            ? "mt-1 text-xs text-destructive"
            : "mt-1 text-xs text-muted-foreground"
        }
      >
        {transferStatus(transfer)}
      </p>
    </div>
  );
}

/** 云盘：账户在服务端的文件。从这台电脑存进去的，别的设备也取得到。 */
function DriveCard({
  source,
  snapshot,
  onError,
  query,
}: CardProps & { query: string }) {
  const [pending, setPending] = useState(false);
  const [confirming, setConfirming] = useState<string | null>(null);
  // 名单由 Rust 在连上服务时去拿；万一没拿到（比如刚登录时断了一下），这里补要一次。
  const requested = useRef<string | null>(null);
  const signedInAs =
    snapshot?.session.phase === "signedIn" ? snapshot.session.device.id : null;
  const drive = snapshot?.drive ?? null;

  useEffect(() => {
    if (!signedInAs || drive || requested.current === signedInAs) return;
    requested.current = signedInAs;
    source.driveRefresh().catch((error: unknown) => {
      onError(messageFor(toBridgeError(error)));
    });
  }, [signedInAs, drive, source, onError]);

  if (snapshot == null || snapshot.session.phase !== "signedIn") return null;

  async function act(what: () => Promise<unknown>) {
    setPending(true);
    onError(null);
    try {
      await what();
    } catch (error: unknown) {
      onError(messageFor(toBridgeError(error)));
    } finally {
      setPending(false);
      setConfirming(null);
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>云盘</CardTitle>
        <CardDescription>
          {drive
            ? `已用 ${formatSpace(drive.usedBytes)}，共 ${formatSpace(drive.limitBytes)}`
            : "正在读取云盘…"}
        </CardDescription>
        <CardAction>
          <Button
            size="sm"
            variant="outline"
            disabled={pending}
            onClick={() => void act(() => source.driveUpload())}
          >
            上传…
          </Button>
        </CardAction>
      </CardHeader>
      {drive ? (
        <CardContent className="flex flex-col gap-2">
          <Progress
            role="progressbar"
            aria-label="云盘空间"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(
              (drive.usedBytes / drive.limitBytes) * 100,
            )}
            value={(drive.usedBytes / drive.limitBytes) * 100}
          />
          {drive.items.length === 0 ? (
            <Empty>
              <EmptyHeader>
                <EmptyTitle>云盘里还没有文件。</EmptyTitle>
                <EmptyDescription>
                  上传文件后，同一账户的其他设备也能取回。
                </EmptyDescription>
              </EmptyHeader>
            </Empty>
          ) : drive.items.filter((file) =>
              file.name
                .toLocaleLowerCase()
                .includes(query.trim().toLocaleLowerCase()),
            ).length === 0 ? (
            <Empty>
              <EmptyHeader>
                <EmptyTitle>没有匹配的文件</EmptyTitle>
                <EmptyDescription>换个关键词试试。</EmptyDescription>
              </EmptyHeader>
            </Empty>
          ) : (
            drive.items
              .filter((file) =>
                file.name
                  .toLocaleLowerCase()
                  .includes(query.trim().toLocaleLowerCase()),
              )
              .map((file) => (
                <DriveRow
                  key={file.id}
                  source={source}
                  file={file}
                  confirming={confirming === file.id}
                  pending={pending}
                  onConfirm={() => setConfirming(file.id)}
                  onCancel={() => setConfirming(null)}
                  act={act}
                />
              ))
          )}
        </CardContent>
      ) : null}
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
    <div className="flex flex-wrap items-start gap-3 rounded-lg border border-border px-3 py-2">
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium" title={file.name}>
          {file.name}
        </p>
        <p className="text-xs text-muted-foreground">
          {formatSize(file.size)} · {file.createdAt.slice(0, 10)}
        </p>
      </div>
      <div className="flex shrink-0 flex-wrap gap-2">
        {confirming ? (
          <>
            <Button
              size="sm"
              variant="destructive"
              disabled={pending}
              onClick={() => void act(() => source.driveDelete(file.id))}
            >
              确认删除
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={onCancel}
            >
              取消
            </Button>
          </>
        ) : (
          <>
            <Button
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={() => void act(() => source.driveDownload(file.id))}
            >
              取回
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={onConfirm}
            >
              删除
            </Button>
          </>
        )}
      </div>
    </div>
  );
}

const LAUNCH_HINTS: Record<LaunchState, string | null> = {
  enabled: null,
  disabled: null,
  unsupported: "开发运行不设置开机启动。",
  unknown: "等待系统批准后生效。",
};

/** 默认的接收位置。它是一个固定的系统位置，不是哪个具体文件夹的路径。 */
const DEFAULT_RECEIVE = "下载/Assemble";

function LaunchCard({ source, snapshot, onError }: CardProps) {
  const [pending, setPending] = useState(false);
  const state: LaunchState = snapshot?.launchAtLogin ?? "unknown";
  const hint = LAUNCH_HINTS[state];
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
      onError(messageFor(toBridgeError(error)));
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
      onError(messageFor(toBridgeError(error)));
    } finally {
      setPending(false);
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>本机</CardTitle>
        <CardDescription>开机启动</CardDescription>
        <CardAction>
          <Switch
            aria-label="开机启动"
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
            <p className="text-sm font-medium">接收位置</p>
            <p
              className={cn(
                "text-sm text-muted-foreground",
                receivePath === null ? "truncate" : "break-all select-text",
              )}
            >
              {receivePath ??
                (customFolder === null
                  ? DEFAULT_RECEIVE
                  : `「${customFolder}」文件夹`)}
            </p>
          </div>
          <div className="flex shrink-0 flex-wrap gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={() => void locate(() => source.chooseReceiveFolder())}
            >
              更改…
            </Button>
            {customFolder !== null ? (
              <Button
                size="sm"
                variant="outline"
                disabled={pending}
                onClick={() => void locate(() => source.resetReceiveFolder())}
              >
                恢复默认
              </Button>
            ) : null}
          </div>
        </div>
        {/* 提醒有没有用，要看登录没登录：没登录就收不到，这一项也不必摆出来。 */}
        {snapshot?.session.phase === "signedIn" ? (
          <div className="flex flex-wrap items-center justify-between gap-3 pt-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">到点提醒</p>
              <p className="text-sm text-muted-foreground">
                {snapshot.plannerMuted
                  ? "这台电脑不弹提醒，到点的只留在日程里。"
                  : "日程和提醒到点时，这台电脑会弹系统通知。"}
              </p>
            </div>
            <Switch
              aria-label="到点时弹提醒"
              checked={!snapshot.plannerMuted}
              disabled={pending}
              onCheckedChange={(checked) =>
                void flip(() => source.setPlannerMuted(!checked))
              }
            />
          </div>
        ) : null}
        {sendMenu === "unsupported" ? null : (
          <div className="flex items-center justify-between gap-3 border-t border-border pt-3">
            <div className="min-w-0">
              <p className="text-sm font-medium">右键菜单</p>
              <p className="text-sm text-muted-foreground">
                {sendMenu === "enabled"
                  ? "在文件上点右键，选择「用 Assemble 发送」。"
                  : "文件的右键菜单里没有「用 Assemble 发送」。"}
              </p>
            </div>
            <Switch
              aria-label="右键菜单里的“用 Assemble 发送”"
              checked={sendMenu === "enabled"}
              disabled={pending}
              onCheckedChange={(checked) =>
                void flip(() => source.setSendMenu(checked))
              }
            />
          </div>
        )}
      </CardContent>
    </Card>
  );
}
