// 应用主界面。
//
// 本阶段只有两类真实内容：账户与连接、本机设置。设备、文件传输与日程的视图
// 在对应的后台能力接通之后再加入，不放空页面占位。

import { useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import { useSnapshot } from "@/hooks/use-snapshot";
import { type Bridge, bridge, toBridgeError } from "@/lib/bridge";
import { messageFor } from "@/lib/messages";
import type {
  LaunchState,
  Notice,
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
        <ServiceStatus service={snapshot?.service} />
      </header>

      {loadError ? (
        <p role="alert" className="text-sm text-destructive">
          {loadError}
        </p>
      ) : null}
      {snapshot?.notice ? <NoticeLine notice={snapshot.notice} /> : null}
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

/** 服务是否可达。还没探测出来时如实显示"正在检测"，不猜。 */
function ServiceStatus({ service }: { service: ServiceView | undefined }) {
  if (!service || service.reachable === null) {
    return <p className="text-sm text-muted-foreground">正在检测连接</p>;
  }
  const name = SERVICE_NAMES[service.target];
  if (service.reachable) {
    return <p className="text-sm text-muted-foreground">{name}</p>;
  }
  return <p className="text-sm text-destructive">{name}暂不可达</p>;
}

function NoticeLine({ notice }: { notice: Notice }) {
  const isError = notice.kind === "error";
  return (
    <p
      role={isError ? "alert" : "status"}
      className={
        isError ? "text-sm text-destructive" : "text-sm text-muted-foreground"
      }
    >
      {notice.message}
    </p>
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
