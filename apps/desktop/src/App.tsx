// 应用主界面。
//
// 现在有四类真实内容：账户与连接、账户里的设备、文件传输、本机设置。
// 日程的视图在对应的后台能力接通之后再加入，不放空页面占位。

import { ArrowDownToLine, ArrowUpToLine } from "lucide-react";
import { useState } from "react";

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
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { Switch } from "@/components/ui/switch";
import { useSnapshot } from "@/hooks/use-snapshot";
import { type Bridge, bridge, toBridgeError } from "@/lib/bridge";
import { messageFor } from "@/lib/messages";
import type {
  DeviceView,
  LaunchState,
  Notice,
  Platform,
  ServiceTarget,
  ServiceView,
  Snapshot,
  TransferFailure,
  TransferView,
} from "@/lib/types";

export interface AppProps {
  /** 默认使用真实的桥接；测试时注入替身。 */
  bridge?: Bridge;
}

export function App({ bridge: source = bridge }: AppProps) {
  const { snapshot, loadError } = useSnapshot(source);
  const [actionError, setActionError] = useState<string | null>(null);

  return (
    <main className="mx-auto flex min-h-screen w-full max-w-xl flex-col gap-4 bg-background px-6 py-6 text-foreground">
      <header className="flex items-center justify-between gap-4 border-b pb-3">
        <h1 className="text-base leading-none font-semibold">Assemble</h1>
        <ServiceStatus
          service={snapshot?.service}
          signedIn={snapshot?.session.phase === "signedIn"}
        />
      </header>

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
      <TransfersCard
        source={source}
        snapshot={snapshot}
        onError={setActionError}
      />
      <LaunchCard
        source={source}
        snapshot={snapshot}
        onError={setActionError}
      />
    </main>
  );
}

/** 服务的显示名。文案属于界面，Rust 只给出连接的是哪一套服务。 */
const SERVICE_NAMES: Record<ServiceTarget, string> = {
  production: "正式服务",
  development: "本地开发服务",
};

/** 服务是否可达、事件流是否连着。还没探测出来时如实显示"正在检测"，不猜。 */
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
  const name = SERVICE_NAMES[service.target];
  if (!service.reachable) {
    return (
      <p className="text-sm text-destructive">
        {signedIn ? "连接已断开，正在重连" : `${name}暂不可达`}
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
  return <p className="text-sm text-muted-foreground">{name}</p>;
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
      onError(messageFor(toBridgeError(error)));
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
        <CardContent>
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
      const { displayName, username } = session.account;
      return displayName === username
        ? username
        : `${displayName}（${username}）`;
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
      <div className="flex items-center gap-3">
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
          <div className="flex shrink-0 gap-2">
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

/** 文件大小怎么显示。单文件不超过 100 MB，所以最大到 MB 就够了。 */
function formatSize(bytes: number): string {
  const kilobytes = bytes / 1024;
  if (kilobytes < 1) return `${bytes} B`;
  if (kilobytes < 1024)
    return `${kilobytes < 10 ? kilobytes.toFixed(1) : Math.round(kilobytes)} KB`;
  return `${(kilobytes / 1024).toFixed(1)} MB`;
}

const FAILURE_NAMES: Record<TransferFailure, string> = {
  diskFull: "这台电脑的磁盘已满",
  writeFailed: "文件没能写进接收文件夹",
  verificationFailed: "收到的内容与发出的对不上",
  cancelled: "已取消",
};

/** 一条传输现在怎么样了，用一句话说清楚。 */
function transferStatus(transfer: TransferView): string {
  switch (transfer.phase) {
    case "queued":
      return "等待接收…";
    case "inProgress":
      return `正在接收 ${Math.floor(transfer.progress * 100)}%`;
    case "complete":
      return "已保存";
    case "failed":
      return `接收失败：${transfer.failure ? FAILURE_NAMES[transfer.failure] : "原因不明"}。`;
    case "cancelled":
      return "对方撤回了，没有收到。";
    case "expired":
      return "过了时限，没有收到。";
  }
}

/** 文件传输。别的设备发来的文件会自动存到接收位置，这里看得到每一次的结果。 */
function TransfersCard({ source, snapshot, onError }: CardProps) {
  if (snapshot?.session.phase !== "signedIn") return null;

  return (
    <Card>
      <CardHeader>
        <CardTitle>传输</CardTitle>
        <CardDescription>
          别的设备发来的文件会自动存到接收位置，收到后有通知。
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-2">
        {snapshot.transfers.length === 0 ? (
          <p className="text-sm text-muted-foreground">还没有收到过文件。</p>
        ) : (
          snapshot.transfers.map((transfer) => (
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

  // 只在文件夹里选中它，不打开：收到的东西要不要打开由用户自己决定。
  async function reveal() {
    setPending(true);
    onError(null);
    try {
      await source.revealReceived(transfer.id);
    } catch (error: unknown) {
      onError(messageFor(toBridgeError(error)));
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="rounded-lg border border-border px-3 py-2">
      <div className="flex items-start gap-3">
        <span
          aria-hidden
          className="mt-0.5 text-muted-foreground"
          data-direction={transfer.direction}
        >
          {transfer.direction === "in" ? (
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
            {transfer.peer} · {formatSize(transfer.size)}
          </p>
          {transfer.savedAs ? (
            <p className="text-xs text-muted-foreground">
              已存为「{transfer.savedAs}」
            </p>
          ) : null}
        </div>
        {transfer.phase === "complete" ? (
          <Button
            size="sm"
            variant="outline"
            className="shrink-0"
            disabled={pending}
            onClick={() => void reveal()}
          >
            在文件夹中显示
          </Button>
        ) : null}
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

  async function toggle(enabled: boolean) {
    setPending(true);
    onError(null);
    try {
      await source.setLaunchAtLogin(enabled);
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
            onCheckedChange={(checked) => void toggle(checked)}
          />
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {hint ? <p className="text-sm text-muted-foreground">{hint}</p> : null}
        <div className="flex items-center justify-between gap-3 border-t border-border pt-3">
          <div className="min-w-0">
            <p className="text-sm font-medium">接收位置</p>
            <p className="truncate text-sm text-muted-foreground">
              {customFolder === null
                ? DEFAULT_RECEIVE
                : `「${customFolder}」文件夹`}
            </p>
          </div>
          <div className="flex shrink-0 gap-2">
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
      </CardContent>
    </Card>
  );
}
