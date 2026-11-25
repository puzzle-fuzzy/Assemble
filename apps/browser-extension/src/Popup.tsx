import { Badge } from "@assemble/ui/badge";
import { Button } from "@assemble/ui/button";
import { DisplayPreferencesItems } from "@assemble/ui/display-preferences";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@assemble/ui/dropdown-menu";
import { ScrollArea } from "@assemble/ui/scroll-area";
import { Download, RefreshCw, Settings2 } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { errorText, t, useBrowserTranslation } from "./i18n";
import type { PopupReply, PopupRequest, SourceList } from "./protocol";

export interface PopupProps {
  send: (request: PopupRequest) => Promise<PopupReply>;
  requestCapture: () => Promise<boolean>;
}
export function Popup({ send, requestCapture }: PopupProps) {
  useBrowserTranslation();
  const [list, setList] = useState<SourceList>({ items: [], capturing: false });
  const [pending, setPending] = useState<string | null>("scan");
  const [error, setError] = useState<string | null>(null);
  const lock = useRef(false);
  const run = useCallback(
    async (request: PopupRequest) => {
      if (lock.current) return;
      lock.current = true;
      setPending(request.type === "download" ? request.id : request.type);
      setError(null);
      try {
        const reply = await send(request);
        if (reply.ok) {
          if (reply.list) setList(reply.list);
        } else setError(reply.code);
      } catch {
        setError("BROWSER_NOT_CONNECTED");
      } finally {
        lock.current = false;
        setPending(null);
      }
    },
    [send],
  );
  useEffect(() => {
    void run({ type: "scan" });
  }, [run]);
  useEffect(() => {
    const refresh = setInterval(() => {
      if (list.capturing && !lock.current)
        void (async () => {
          try {
            const reply = await send({ type: "list" });
            if (reply.ok && reply.list && !lock.current) setList(reply.list);
          } catch {
            /* 下次轮询或用户刷新时重试，不覆盖下载失败提示。 */
          }
        })();
    }, 1500);
    return () => clearInterval(refresh);
  }, [list.capturing, send]);
  async function capture() {
    if (list.capturing) {
      await run({ type: "capture", enabled: false });
      return;
    }
    try {
      const granted = await requestCapture();
      if (!granted) {
        setError("PERMISSION_DENIED");
        return;
      }
      await run({ type: "capture", enabled: true });
    } catch {
      setError("PERMISSION_DENIED");
    }
  }
  return (
    <main className="flex w-[380px] max-w-full flex-col gap-3 p-4">
      <header className="flex items-center justify-between gap-2">
        <h1 className="text-sm font-semibold">Assemble</h1>
        <div className="flex gap-1">
          <Button
            size="sm"
            variant="outline"
            disabled={pending !== null}
            aria-pressed={list.capturing}
            onClick={() => void capture()}
          >
            {t(list.capturing ? "capturing" : "capture")}
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={t("refresh")}
            disabled={pending !== null}
            onClick={() => void run({ type: "scan" })}
          >
            <RefreshCw aria-hidden="true" />
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={t("preferences")}
              >
                <Settings2 aria-hidden="true" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DisplayPreferencesItems />
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </header>
      {error ? (
        <p role="alert" className="text-sm text-destructive">
          {errorText(error)}
        </p>
      ) : null}
      <ScrollArea className="max-h-[440px]">
        {list.items.length === 0 ? (
          <p
            role="status"
            className="py-8 text-center text-sm text-muted-foreground"
          >
            {t(pending === "scan" ? "scanning" : "empty")}
          </p>
        ) : (
          <ul className="divide-y">
            {list.items.map((source) => (
              <li key={source.id} className="flex items-center gap-3 py-3">
                <div className="min-w-0 flex-1">
                  <p
                    className="truncate text-sm font-medium"
                    title={source.title}
                  >
                    {source.title}
                  </p>
                  <div className="mt-1 flex items-center gap-2">
                    <Badge variant="secondary">{t(source.kind)}</Badge>
                    <span className="truncate text-xs text-muted-foreground">
                      {source.host}
                    </span>
                  </div>
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={pending !== null || source.submitted}
                  aria-label={`${t(source.submitted ? "submitted" : pending === source.id ? "adding" : "download")} ${source.title}`}
                  onClick={() => void run({ type: "download", id: source.id })}
                >
                  <Download data-icon="inline-start" aria-hidden="true" />
                  {t(
                    source.submitted
                      ? "submitted"
                      : pending === source.id
                        ? "adding"
                        : "download",
                  )}
                </Button>
              </li>
            ))}
          </ul>
        )}
      </ScrollArea>
    </main>
  );
}
