// 应用主界面。
//
// 本阶段有三类真实内容：账户与连接、账户里的设备、本机设置。文件传输与日程的视图
// 在对应的后台能力接通之后再加入，不放空页面占位。

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

const LAUNCH_HINTS: Record<LaunchState, string | null> = {
  enabled: null,
  disabled: null,
  unsupported: "开发运行不设置开机启动。",
  unknown: "等待系统批准后生效。",
};

function LaunchCard({ source, snapshot, onError }: CardProps) {
  const [pending, setPending] = useState(false);
  const state: LaunchState = snapshot?.launchAtLogin ?? "unknown";
  const hint = LAUNCH_HINTS[state];

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
      {hint ? <CardContent>{hint}</CardContent> : null}
    </Card>
  );
}
