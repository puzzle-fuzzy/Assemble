import { Alert, AlertDescription } from "@assemble/ui/alert";
import { Button } from "@assemble/ui/button";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@assemble/ui/empty";
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
} from "@assemble/ui/field";
import { Input } from "@assemble/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@assemble/ui/select";
import {
  Maximize,
  Minimize,
  Monitor,
  Send,
  Square,
  Upload,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { toast } from "sonner";
import type {
  RemoteBridge,
  RemoteFrame,
  RemoteInput,
  RemoteSession,
  RemoteView,
} from "@/lib/remote";
import {
  remoteEnded,
  remoteError,
  remoteText,
  useRemoteTranslation,
} from "@/lib/remote-i18n";
import { RemoteInputQueue, remoteKey, remotePoint } from "@/lib/remote-input";
import type { DeviceView } from "@/lib/types";

export function RemoteBanner({
  source,
  view,
  ownId,
  onOpen,
}: {
  source: RemoteBridge;
  view: RemoteView;
  ownId: string | null;
  onOpen: () => void;
}) {
  const { t } = useRemoteTranslation();
  const [pending, setPending] = useState(false);
  const incoming = view.sessions.find(
    (s) => s.hostId === ownId && s.phase !== "ended",
  );
  if (!incoming) return null;
  const session = incoming;
  async function respond(allow: boolean) {
    if (pending) return;
    setPending(true);
    try {
      if (allow) await source.approve(session.id);
      else await source.end(session.id);
    } catch (error) {
      toast.error(remoteError(error));
    } finally {
      setPending(false);
    }
  }
  return (
    <Alert>
      <Monitor aria-hidden="true" />
      <AlertDescription className="flex min-w-0 flex-wrap items-center justify-between gap-3">
        <span className="min-w-0 break-words">
          <strong>{session.controllerName}</strong>{" "}
          {t(session.phase === "pending" ? "incoming" : "hosting")}
        </span>
        <div className="flex flex-wrap gap-2">
          {session.phase === "pending" ? (
            <>
              <Button
                size="sm"
                disabled={pending}
                onClick={() => void respond(false)}
                variant="outline"
              >
                {t("deny")}
              </Button>
              <Button
                size="sm"
                disabled={pending}
                onClick={() => void respond(true)}
              >
                {t("allow")}
              </Button>
            </>
          ) : (
            <>
              <Button size="sm" variant="outline" onClick={onOpen}>
                {t("title")}
              </Button>
              <Button
                size="sm"
                disabled={pending}
                variant="destructive"
                onClick={() => void respond(false)}
              >
                {t("stop")}
              </Button>
            </>
          )}
        </div>
      </AlertDescription>
    </Alert>
  );
}

export function RemotePage({
  source,
  view,
  devices,
  ownId,
  onSendFiles,
}: {
  source: RemoteBridge;
  view: RemoteView;
  devices: DeviceView[];
  ownId: string | null;
  onSendFiles: () => void;
}) {
  const { t } = useRemoteTranslation();
  const [local, setLocal] = useState<RemoteSession | null>(null);
  const [closed, setClosed] = useState<string[]>([]);
  const [pending, setPending] = useState(false);
  const [feedback, setFeedback] = useState<{
    message: string;
    failed: boolean;
  } | null>(null);
  const session = [...view.sessions, ...(local ? [local] : [])].find(
    (s) =>
      s.controllerId === ownId && s.phase !== "ended" && !closed.includes(s.id),
  );
  async function connect(hostId: string) {
    if (pending) return;
    setPending(true);
    setFeedback(null);
    try {
      setLocal(await source.request(hostId));
    } catch (error) {
      setFeedback({ message: remoteError(error), failed: true });
    } finally {
      setPending(false);
    }
  }
  if (!ownId)
    return (
      <Empty>
        <EmptyHeader>
          <EmptyTitle>{t("signIn")}</EmptyTitle>
        </EmptyHeader>
      </Empty>
    );
  if (session)
    return (
      <RemoteController
        key={session.id}
        source={source}
        session={session}
        onSendFiles={onSendFiles}
        onEnded={(message, failed) => {
          setClosed((ids) => [...ids, session.id]);
          setLocal(null);
          if (message) setFeedback({ message, failed: failed ?? true });
        }}
      />
    );
  const peers = devices.filter(
    (d) =>
      d.id !== ownId && (d.platform === "windows" || d.platform === "macos"),
  );
  return (
    <div className="flex min-w-0 flex-col gap-4">
      {feedback ? (
        <Alert
          variant={feedback.failed ? "destructive" : "default"}
          role={feedback.failed ? "alert" : "status"}
        >
          <AlertDescription>{feedback.message}</AlertDescription>
        </Alert>
      ) : null}
      {peers.length ? (
        <p className="text-sm text-muted-foreground">{t("intro")}</p>
      ) : null}
      {peers.length ? (
        <ul className="divide-y rounded-lg border">
          {peers.map((d) => (
            <li key={d.id} className="flex min-w-0 items-center gap-3 p-4">
              <Monitor
                aria-hidden="true"
                className="size-5 shrink-0 text-muted-foreground"
              />
              <div className="min-w-0 flex-1">
                <div className="truncate font-medium">{d.name}</div>
                <div className="text-sm text-muted-foreground">
                  {d.platform === "macos" ? "macOS" : "Windows"}
                  {!d.online ? ` · ${t("offline")}` : ""}
                </div>
              </div>
              <Button
                size="sm"
                variant="outline"
                disabled={!d.online || pending || view.sessions.length > 0}
                onClick={() => void connect(d.id)}
                aria-label={`${t("connect")} ${d.name}`}
              >
                {t("connect")}
              </Button>
            </li>
          ))}
        </ul>
      ) : (
        <Empty>
          <EmptyHeader>
            <EmptyTitle>{t("empty")}</EmptyTitle>
            <EmptyDescription>{t("intro")}</EmptyDescription>
          </EmptyHeader>
        </Empty>
      )}
    </div>
  );
}

function RemoteController({
  source,
  session,
  onEnded,
  onSendFiles,
}: {
  source: RemoteBridge;
  session: RemoteSession;
  onEnded: (error?: string, failed?: boolean) => void;
  onSendFiles: () => void;
}) {
  const { t } = useRemoteTranslation();
  const [frame, setFrame] = useState<RemoteFrame | null>(null);
  const [active, setActive] = useState(session.phase === "active");
  const activeRef = useRef(active);
  activeRef.current = active;
  const [full, setFull] = useState(false);
  const [text, setText] = useState("");
  const [switching, setSwitching] = useState<number | null>(null);
  const surface = useRef<HTMLDivElement>(null);
  const picture = useRef<HTMLImageElement>(null);
  const keys = useRef(new Map<string, string>());
  const pointer = useRef<number | null>(null);
  const wheel = useRef({ x: 0, y: 0 });
  const ended = useRef(false);
  const onEndedRef = useRef(onEnded);
  onEndedRef.current = onEnded;
  const textId = useId();
  const hintId = useId();
  const lifecycle = useRef(0);
  const failed = useRef<() => void>(() => {});
  const queue = useMemo(
    () =>
      new RemoteInputQueue(
        (events) => source.inputs(session.id, events),
        () => failed.current(),
      ),
    [source, session.id],
  );
  const release = useCallback(() => {
    keys.current.clear();
    pointer.current = null;
    wheel.current = { x: 0, y: 0 };
    if (activeRef.current && !ended.current) queue.push({ type: "releaseAll" });
  }, [queue]);
  const finish = useCallback(
    (error?: string, failed = true) => {
      if (ended.current) return;
      ended.current = true;
      queue.stop();
      void source.end(session.id).catch(() => {});
      onEndedRef.current(error, failed);
    },
    [source, session.id, queue],
  );
  failed.current = () => finish(remoteText("failed"));
  useEffect(() => {
    const node = surface.current;
    if (!node) return;
    const scroll = (event: WheelEvent) => {
      if (!activeRef.current || document.activeElement !== node) return;
      event.preventDefault();
      const scale =
        event.deltaMode === 1 ? 40 : event.deltaMode === 2 ? 400 : 1;
      wheel.current.x += event.deltaX * scale;
      wheel.current.y += event.deltaY * scale;
      const x = Math.max(-20, Math.min(20, Math.trunc(wheel.current.x / 40)));
      const y = Math.max(-20, Math.min(20, Math.trunc(wheel.current.y / 40)));
      if (x || y) {
        wheel.current.x -= x * 40;
        wheel.current.y -= y * 40;
        queue.push({ type: "scroll", x, y });
      }
    };
    node.addEventListener("wheel", scroll, { passive: false });
    return () => node.removeEventListener("wheel", scroll);
  }, [queue]);
  useEffect(() => {
    const epoch = ++lifecycle.current;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let after = 0;
    async function poll() {
      try {
        const result = await source.poll(session.id, after);
        if (disposed) return;
        if (result.session.phase === "ended") {
          finish(
            remoteEnded(result.session),
            !["stopped", "declined", "expired"].includes(
              result.session.endReason ?? "stopped",
            ),
          );
          return;
        }
        setActive(result.session.phase === "active");
        if (result.frame) {
          after = result.frame.sequence;
          setFrame(result.frame);
          setSwitching((id) => (id === result.frame?.displayId ? null : id));
        }
        timer = setTimeout(() => void poll(), 125);
      } catch {
        if (!disposed) finish(remoteText("failed"));
      }
    }
    void poll();
    const blur = () => release();
    const visibility = () => {
      if (document.hidden) release();
    };
    window.addEventListener("blur", blur);
    document.addEventListener("visibilitychange", visibility);
    return () => {
      disposed = true;
      clearTimeout(timer);
      window.removeEventListener("blur", blur);
      document.removeEventListener("visibilitychange", visibility);
      queueMicrotask(() => {
        if (lifecycle.current !== epoch) return;
        queue.stop();
        void source.end(session.id).catch(() => {});
        void source.fullscreen(false).catch(() => {});
      });
    };
  }, [source, session.id, queue, release, finish]);
  async function fullscreen() {
    release();
    try {
      await source.fullscreen(!full);
      setFull(!full);
    } catch (error) {
      toast.error(remoteError(error));
    }
  }
  function move(event: { clientX: number; clientY: number }) {
    if (
      !frame ||
      switching !== null ||
      !active ||
      document.activeElement !== surface.current
    )
      return false;
    const bounds = picture.current?.getBoundingClientRect();
    const input = bounds
      ? remotePoint(event.clientX, event.clientY, bounds)
      : null;
    if (input) queue.push(input);
    return input !== null;
  }
  function button(button: number, pressed: boolean) {
    const name = (["left", "middle", "right"] as const)[button];
    if (name) queue.push({ type: "button", button: name, pressed });
  }
  function send(input: RemoteInput) {
    if (active) queue.push(input);
  }
  return (
    <div className="flex min-w-0 flex-col gap-3">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <h2 className="min-w-0 flex-1 truncate font-medium">
          {session.hostName}
        </h2>
        {frame && frame.displays.length > 1 ? (
          <Select
            value={String(switching ?? frame.displayId)}
            onValueChange={(value) => {
              release();
              const id = Number(value);
              setSwitching(id);
              send({ type: "display", id });
            }}
          >
            <SelectTrigger className="w-44" aria-label={t("display")}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                {frame.displays.map((d) => (
                  <SelectItem key={d.id} value={String(d.id)}>
                    {d.name}
                  </SelectItem>
                ))}
              </SelectGroup>
            </SelectContent>
          </Select>
        ) : null}
        <Button size="sm" variant="outline" onClick={onSendFiles}>
          <Upload data-icon="inline-start" />
          {t("files")}
        </Button>
        <Button
          size="sm"
          variant="outline"
          onClick={() => {
            send({ type: "key", key: "Escape", pressed: true });
            send({ type: "key", key: "Escape", pressed: false });
          }}
          disabled={!active}
        >
          {t("escape")}
        </Button>
        <Button size="sm" variant="outline" onClick={() => void fullscreen()}>
          {full ? (
            <Minimize data-icon="inline-start" />
          ) : (
            <Maximize data-icon="inline-start" />
          )}
          {t(full ? "exitFullscreen" : "fullscreen")}
        </Button>
        <Button size="sm" variant="destructive" onClick={() => finish()}>
          <Square data-icon="inline-start" />
          {t("stop")}
        </Button>
      </div>
      <p id={hintId} className="text-sm text-muted-foreground">
        {active ? t("keyboardHint") : t("pending")}
      </p>
      {/* 远程输入没有对应的原生 HTML 控件；Esc 始终返回本机，避免键盘被困在画面里。 */}
      <div
        ref={surface}
        role="application"
        tabIndex={active && frame ? 0 : -1}
        aria-label={t("screen")}
        aria-describedby={hintId}
        className="flex min-h-64 min-w-0 items-center justify-center overflow-hidden rounded-lg border bg-muted focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
        onContextMenu={(e) => e.preventDefault()}
        onBlur={release}
        onPointerMove={move}
        onPointerDown={(e) => {
          if (!active || !frame || switching !== null) return;
          e.preventDefault();
          surface.current?.focus();
          if (!move(e)) return;
          pointer.current = e.pointerId;
          e.currentTarget.setPointerCapture(e.pointerId);
          button(e.button, true);
        }}
        onPointerUp={(e) => {
          if (!active || pointer.current !== e.pointerId) return;
          move(e);
          button(e.button, false);
          pointer.current = null;
          if (e.currentTarget.hasPointerCapture(e.pointerId))
            e.currentTarget.releasePointerCapture(e.pointerId);
        }}
        onPointerCancel={release}
        onLostPointerCapture={() => {
          if (pointer.current !== null) release();
        }}
        onKeyDown={(e) => {
          if (!active) return;
          if (e.key === "Escape") {
            e.preventDefault();
            release();
            e.currentTarget.blur();
            if (full) void fullscreen();
            return;
          }
          if (e.nativeEvent.isComposing) return;
          const key = remoteKey(e.key, e.code);
          if (!key) return;
          e.preventDefault();
          if (keys.current.has(e.code)) {
            if (e.repeat && !["Control", "Shift", "Alt", "Meta"].includes(key))
              send({ type: "key", key, pressed: true });
            return;
          }
          const alreadyPressed = [...keys.current.values()].includes(key);
          keys.current.set(e.code, key);
          if (!alreadyPressed) send({ type: "key", key, pressed: true });
        }}
        onKeyUp={(e) => {
          const key = keys.current.get(e.code);
          if (!key) return;
          e.preventDefault();
          keys.current.delete(e.code);
          if (![...keys.current.values()].includes(key))
            send({ type: "key", key, pressed: false });
        }}
      >
        {frame ? (
          <img
            ref={picture}
            src={`data:image/jpeg;base64,${frame.jpeg}`}
            alt={t("screen")}
            width={frame.width}
            height={frame.height}
            draggable={false}
            className="block max-h-[70vh] max-w-full select-none object-contain"
            onError={() => finish(t("failed"))}
          />
        ) : (
          <p className="p-6 text-sm text-muted-foreground">
            {active ? t("waitingFrame") : t("pending")}
          </p>
        )}
      </div>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (!text.trim() || !active) return;
          release();
          send({ type: "text", text });
          setText("");
        }}
      >
        <FieldGroup>
          <Field>
            <FieldLabel htmlFor={textId}>{t("text")}</FieldLabel>
            <div className="flex min-w-0 gap-2">
              <Input
                id={textId}
                value={text}
                onChange={(e) => setText(e.target.value)}
                maxLength={1000}
                disabled={!active}
              />
              <Button
                type="submit"
                size="sm"
                variant="outline"
                disabled={!active || !text.trim()}
              >
                <Send data-icon="inline-start" />
                {t("sendText")}
              </Button>
            </div>
            <FieldDescription>{t("textHint")}</FieldDescription>
          </Field>
        </FieldGroup>
      </form>
    </div>
  );
}
