// 发送面板：挑好文件之后，选一台设备发过去。
//
// 它是一个单独的窗口，只认标识：要发的文件、账户里的设备、每一条传输。
// 文件在哪、怎么上传，都在 Rust 那边。挑文件、选设备、发出去，点三下就结束。

import { DisplayPreferencesMenu } from "@assemble/ui/display-preferences";
import { Cloud } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { useSnapshot } from "@/hooks/use-snapshot";
import { type Bridge, bridge, toBridgeError } from "@/lib/bridge";
import { uiText, useDesktopTranslation } from "@/lib/i18n";
import { messageFor } from "@/lib/messages";
import { formatSize, transferStatus, verdict } from "@/lib/transfer-text";
import type { TransferView } from "@/lib/types";

export interface SendPanelProps {
  /** 默认使用真实的桥接；测试时注入替身。 */
  bridge?: Bridge;
}

/** 全部发完之后，让结果停留这么久再收起面板。 */
const LINGER_MS = 1500;

/** 平台的显示名。 */
const PLATFORM_NAMES = { windows: "Windows", macos: "Mac", linux: "Linux" };

export function SendPanel({ bridge: source = bridge }: SendPanelProps) {
  useDesktopTranslation();
  const { snapshot, loadError } = useSnapshot(source);
  // 发起之后盯着的那几条传输。
  const [sent, setSent] = useState<string[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [closing, setClosing] = useState(false);

  // 逐个分支再判断一次"登录了没有"，类型才知道会话里有这台设备自己。
  const signedIn = snapshot?.session.phase === "signedIn";
  const thisDevice =
    signedIn && snapshot.session.phase === "signedIn"
      ? snapshot.session.device.id
      : null;
  const draft = snapshot?.draft ?? null;
  const destinations = signedIn
    ? snapshot.devices.filter((device) => device.id !== thisDevice)
    : [];
  const mine = useMemo(
    () =>
      sent
        ? (snapshot?.transfers ?? []).filter((t) => sent.includes(t.id))
        : [],
    [sent, snapshot],
  );
  const outcome = verdict(mine);
  const panelTitle = sent
    ? outcome === "failed"
      ? uiText("someFilesWereNotSent")
      : uiText("sendingEllipsis")
    : uiText("sendToEllipsis");
  useEffect(() => {
    document.title = `${panelTitle} · Assemble`;
  }, [panelTitle]);

  // 全部传完而且没有一个失败：停一会儿让用户看清，然后收起。
  useEffect(() => {
    if (outcome !== "delivered") return;
    setClosing(true);
    const timer = setTimeout(() => void source.closeSendPanel(), LINGER_MS);
    return () => clearTimeout(timer);
  }, [outcome, source]);

  // 云盘不是一个设备：在面板里它排在设备名单的最前面，永远可以选。
  async function sendTo(destination: string | "drive") {
    setPending(true);
    setProblem(null);
    try {
      setSent(
        destination === "drive"
          ? await source.sendToDrive()
          : await source.sendToDevice(destination),
      );
    } catch (error: unknown) {
      setProblem(toBridgeError(error).code);
    } finally {
      setPending(false);
    }
  }

  async function retryFailed() {
    setPending(true);
    setProblem(null);
    try {
      for (const transfer of mine) {
        if (transfer.phase === "failed" && transfer.retryable) {
          await source.retryTransfer(transfer.id);
        }
      }
    } catch (error: unknown) {
      setProblem(toBridgeError(error).code);
    } finally {
      setPending(false);
    }
  }

  async function close() {
    try {
      await source.closeSendPanel();
    } catch (error: unknown) {
      // 面板关不掉不影响文件：主窗口里还看得到这几条传输。
      console.warn(messageFor(toBridgeError(error)));
    }
  }

  return (
    <main className="flex min-h-screen w-full flex-col gap-3 bg-background px-5 py-5 text-foreground">
      <header className="flex items-center justify-between gap-3">
        <h1 className="text-base leading-none font-semibold">{panelTitle}</h1>
        <DisplayPreferencesMenu />
      </header>
      {loadError ? (
        <p role="alert" className="text-sm text-destructive">
          {loadError}
        </p>
      ) : null}
      {!snapshot ? (
        <p className="text-sm text-muted-foreground">
          {uiText("openingEllipsis")}
        </p>
      ) : null}

      {snapshot && !signedIn ? (
        <>
          <p className="text-sm text-muted-foreground">
            {uiText("signInFromTheMainAssembleWindowToSendFiles")}
          </p>
          <Button variant="outline" onClick={() => void source.openMain()}>
            {uiText("openAssemble")}
          </Button>
        </>
      ) : null}

      {snapshot && signedIn && !draft && !sent ? (
        <>
          <p className="text-sm text-muted-foreground">
            {uiText("noFilesSelected")}
          </p>
          <div className="flex gap-2">
            <Button
              onClick={() => void source.startSend().catch(() => undefined)}
            >
              {uiText("chooseAgain")}
            </Button>
            <Button variant="outline" onClick={() => void close()}>
              {uiText("close")}
            </Button>
          </div>
        </>
      ) : null}

      {draft && !sent ? (
        <>
          <ul className="flex flex-col gap-1 rounded-lg border border-border px-3 py-2">
            {draft.files.map((file) => (
              <li
                key={file.name}
                className="flex justify-between gap-3 text-sm"
              >
                <span className="truncate" title={file.name}>
                  {file.name}
                </span>
                <span className="shrink-0 text-xs text-muted-foreground">
                  {formatSize(file.size)}
                </span>
              </li>
            ))}
          </ul>
          <ul
            className="flex flex-col gap-2"
            aria-label={uiText("chooseADestination")}
          >
            <li>
              <button
                type="button"
                className="flex w-full items-center justify-between gap-3 rounded-lg border border-border px-3 py-2 text-left text-sm hover:bg-muted disabled:cursor-not-allowed disabled:opacity-50"
                disabled={pending}
                onClick={() => void sendTo("drive")}
              >
                <span className="flex min-w-0 items-center gap-2">
                  <Cloud
                    aria-hidden
                    className="size-3.5 shrink-0 text-muted-foreground"
                  />
                  <span className="truncate">{uiText("cloudDrive")}</span>
                </span>
                <span className="shrink-0 text-xs text-muted-foreground">
                  {uiText("storeOnTheServer")}
                </span>
              </button>
            </li>
            {destinations.map((device) => (
              <li key={device.id}>
                <button
                  type="button"
                  className="flex w-full items-center justify-between gap-3 rounded-lg border border-border px-3 py-2 text-left text-sm hover:bg-muted disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={!device.online || pending}
                  onClick={() => void sendTo(device.id)}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <span
                      aria-hidden
                      className={
                        device.online
                          ? "size-2 shrink-0 rounded-full bg-emerald-500"
                          : "size-2 shrink-0 rounded-full bg-muted-foreground/40"
                      }
                    />
                    <span className="truncate">{device.name}</span>
                  </span>
                  <span className="shrink-0 text-xs text-muted-foreground">
                    {PLATFORM_NAMES[device.platform]} ·{" "}
                    {device.online ? uiText("online") : uiText("offline")}
                  </span>
                </button>
              </li>
            ))}
          </ul>
          {problem ? (
            <p role="alert" className="text-sm text-destructive">
              {messageFor(toBridgeError({ code: problem }))}
            </p>
          ) : null}
        </>
      ) : null}

      {sent ? (
        <>
          <ul className="flex flex-col gap-2">
            {mine.map((transfer) => (
              <SentFile key={transfer.id} transfer={transfer} />
            ))}
          </ul>
          {outcome === "failed" ? (
            <>
              {mine.some((t) => t.phase === "failed" && t.retryable) ? (
                <Button
                  variant="outline"
                  disabled={pending}
                  onClick={() => void retryFailed()}
                >
                  {uiText("retryUnsentFiles")}
                </Button>
              ) : null}
              <Button
                variant="outline"
                disabled={pending}
                onClick={() => void close()}
              >
                {uiText("close")}
              </Button>
            </>
          ) : (
            <p className="text-sm text-muted-foreground">
              {closing
                ? uiText("sent")
                : uiText("thisWindowClosesWhenTheTransferFinishes")}
            </p>
          )}
          {problem ? (
            <p role="alert" className="text-sm text-destructive">
              {messageFor(toBridgeError({ code: problem }))}
            </p>
          ) : null}
        </>
      ) : null}
    </main>
  );
}

function SentFile({ transfer }: { transfer: TransferView }) {
  return (
    <li className="rounded-lg border border-border px-3 py-2">
      <p className="truncate text-sm font-medium" title={transfer.name}>
        {transfer.name}
      </p>
      {transfer.phase === "inProgress" ? (
        <Progress
          aria-hidden
          className="mt-2"
          value={transfer.progress * 100}
        />
      ) : null}
      <p
        className={
          transfer.phase === "failed"
            ? "mt-1 text-xs text-destructive"
            : "mt-1 text-xs text-muted-foreground"
        }
      >
        {transferStatus(transfer)}
      </p>
    </li>
  );
}
