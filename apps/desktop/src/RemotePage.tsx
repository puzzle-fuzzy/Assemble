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
import { ToggleGroup, ToggleGroupItem } from "@assemble/ui/toggle-group";
import {
  Circle,
  ClipboardCopy,
  ClipboardPaste,
  Hand,
  Maximize,
  Minimize,
  Monitor,
  Send,
  Square,
  Upload,
  Volume2,
  VolumeX,
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
  CursorUpdate,
  RemoteBridge,
  RemoteFrame,
  RemoteInput,
  RemoteQuality,
  RemoteSession,
  RemoteView,
} from "@/lib/remote";
import { RemoteAudioPlayer } from "@/lib/remote-audio";
import {
  remoteEnded,
  remoteError,
  remoteText,
  useRemoteTranslation,
} from "@/lib/remote-i18n";
import {
  clipboardShortcut,
  RemoteInputQueue,
  remoteKey,
  remotePoint,
} from "@/lib/remote-input";
import { RemoteVideoDecoder } from "@/lib/remote-video";
import type { DeviceView } from "@/lib/types";
import { cn } from "@/lib/utils";

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
          {session.phase === "pending" ? (
            <span className="mt-1 block text-sm">
              {session.verification
                ? t("verify", { code: session.verification })
                : t("securing")}
            </span>
          ) : null}
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
                onClick={() => void respond(true)}
                disabled={pending || !session.verification}
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
  const frameRef = useRef(frame);
  frameRef.current = frame;
  const [cursor, setCursor] = useState<CursorUpdate | null>(null);
  const [pictureBox, setPictureBox] = useState({
    left: 0,
    top: 0,
    width: 0,
    height: 0,
  });
  const mediaKind = frame ? (frame.video ? "video" : "image") : "none";
  const [active, setActive] = useState(session.phase === "active");
  const [recovering, setRecovering] = useState(false);
  const [verification, setVerification] = useState(session.verification);
  const activeRef = useRef(active);
  activeRef.current = active;
  const [full, setFull] = useState(false);
  const [controls, setControls] = useState(true);
  const controlsAt = useRef(0);
  const revealControls = useCallback(() => {
    controlsAt.current = Date.now();
    setControls(true);
  }, []);
  const toolbar = useRef<HTMLDivElement>(null);
  const [text, setText] = useState("");
  const [switching, setSwitching] = useState<number | null>(null);
  const [clipboardBusy, setClipboardBusy] = useState(false);
  const clipboardLock = useRef(false);
  const [scaling, setScaling] = useState<"fit" | "actual">("fit");
  const [browsing, setBrowsing] = useState(false);
  const browsingRef = useRef(false);
  browsingRef.current = browsing;
  const [quality, setQuality] = useState<RemoteQuality>("auto");
  const [qualityPending, setQualityPending] = useState<RemoteQuality | null>(
    null,
  );
  const pan = useRef<{
    id: number;
    x: number;
    y: number;
    left: number;
    top: number;
  } | null>(null);
  const surface = useRef<HTMLDivElement>(null);
  const picture = useRef<HTMLImageElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const decoder = useRef<RemoteVideoDecoder | null>(null);
  const videoPending = useRef<RemoteFrame[]>([]);
  const [sound, setSound] = useState(false);
  const soundWanted = useRef(false);
  const soundOpening = useRef(false);
  const player = useRef<RemoteAudioPlayer | null>(null);
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
    pan.current = null;
    wheel.current = { x: 0, y: 0 };
    if (activeRef.current && !ended.current) queue.release();
  }, [queue]);
  const finish = useCallback(
    (error?: string, failed = true) => {
      if (ended.current) return;
      ended.current = true;
      player.current?.close();
      player.current = null;
      soundWanted.current = false;
      queue.stop();
      void source.end(session.id).catch(() => {});
      onEndedRef.current(error, failed);
    },
    [source, session.id, queue],
  );
  failed.current = () => finish(remoteText("failed"));
  useEffect(() => {
    const node = mediaKind === "video" ? canvas.current : picture.current;
    if (!node || !surface.current) return;
    const measure = () => {
      const next = {
        left: node.offsetLeft,
        top: node.offsetTop,
        width: node.offsetWidth,
        height: node.offsetHeight,
      };
      setPictureBox((old) =>
        old.left === next.left &&
        old.top === next.top &&
        old.width === next.width &&
        old.height === next.height
          ? old
          : next,
      );
    };
    measure();
    const observer =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(measure);
    observer?.observe(node);
    observer?.observe(surface.current);
    window.addEventListener("resize", measure);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [mediaKind]);
  useEffect(() => {
    if (!full || clipboardBusy) return;
    const timer = setInterval(() => {
      const focused = document.activeElement;
      if (
        Date.now() - controlsAt.current >= 3000 &&
        !toolbar.current?.contains(focused) &&
        !(focused instanceof HTMLInputElement)
      )
        setControls(false);
    }, 250);
    return () => clearInterval(timer);
  }, [full, clipboardBusy]);
  useEffect(() => {
    if (switching === null) return;
    const timer = setTimeout(() => {
      setSwitching(null);
      toast.error(remoteText("displayUnavailable"));
    }, 3000);
    return () => clearTimeout(timer);
  }, [switching]);
  useEffect(() => {
    if (!active) return;
    let disposed = false;
    void RemoteVideoDecoder.supported().then((supported) => {
      if (disposed || !supported) return;
      try {
        decoder.current = new RemoteVideoDecoder(
          () => canvas.current,
          () => queue.push({ type: "keyframe" }),
          () => {
            queue.push({ type: "video", enabled: false });
            toast.info(remoteText("videoFallback"));
          },
        );
        queue.push({ type: "video", enabled: true });
      } catch {
        toast.info(remoteText("videoFallback"));
      }
    });
    return () => {
      disposed = true;
      decoder.current?.close();
      decoder.current = null;
    };
  }, [queue, active]);
  useEffect(() => {
    if (!frame?.video) return;
    const pending = videoPending.current.splice(0);
    for (const frame of pending) decoder.current?.receive(frame);
  }, [frame]);
  useEffect(() => {
    if (!sound) return;
    let disposed = false;
    let after = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    function mute() {
      soundWanted.current = false;
      player.current?.close();
      player.current = null;
      setSound(false);
      queue.push({ type: "audio", enabled: false });
    }
    async function pollAudio() {
      try {
        const audio = await source.audio(session.id, after);
        if (disposed || !soundWanted.current) return;
        if (audio.state === "unavailable") {
          mute();
          toast.error(remoteText("audioUnavailable"));
          return;
        }
        player.current?.append(audio);
        after = audio.packets.at(-1)?.sequence ?? after;
        timer = setTimeout(() => void pollAudio(), 30);
      } catch {
        if (!disposed) {
          mute();
          toast.error(remoteText("audioUnavailable"));
        }
      }
    }
    void pollAudio();
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [sound, source, session.id, queue]);
  useEffect(() => {
    const node = surface.current;
    if (!node) return;
    const scroll = (event: WheelEvent) => {
      if (
        !activeRef.current ||
        clipboardLock.current ||
        browsingRef.current ||
        document.activeElement !== node
      )
        return;
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
        queue.push({ type: "scroll", x, y }, frameRef.current?.displayId);
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
    let readFailures = 0;
    async function poll() {
      try {
        const result = await source.poll(session.id, after);
        if (disposed) return;
        readFailures = 0;
        setVerification(result.session.verification);
        if (result.cursor !== undefined) setCursor(result.cursor);
        setRecovering(false);
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
        const frames = result.frames?.length
          ? result.frames
          : result.frame
            ? [result.frame]
            : [];
        if (frames.length) {
          const latest = frames[frames.length - 1];
          if (!latest) return;
          after = latest.sequence;
          videoPending.current = frames;
          setFrame(latest);
          setQuality(latest.requestedQuality ?? "auto");
          setQualityPending((pending) =>
            pending === (latest.requestedQuality ?? "auto") ? null : pending,
          );
          setSwitching((id) => (id === latest.displayId ? null : id));
        }
        timer = setTimeout(() => void poll(), 25);
      } catch (error) {
        if (disposed) return;
        const retryable =
          typeof error === "object" &&
          error !== null &&
          "code" in error &&
          error.code === "UNREACHABLE";
        if (retryable && readFailures < 2) {
          release();
          setActive(false);
          setRecovering(true);
          timer = setTimeout(
            () => void poll(),
            [100, 250][readFailures++] ?? 250,
          );
        } else finish(remoteText("failed"));
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
        player.current?.close();
        player.current = null;
        soundWanted.current = false;
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
      revealControls();
    } catch (error) {
      toast.error(remoteError(error));
    }
  }
  async function toggleSound() {
    if (soundOpening.current || !active || ended.current) return;
    if (soundWanted.current) {
      soundWanted.current = false;
      player.current?.close();
      player.current = null;
      setSound(false);
      queue.push({ type: "audio", enabled: false });
      return;
    }
    soundOpening.current = true;
    const epoch = lifecycle.current;
    try {
      const opened = await RemoteAudioPlayer.open();
      if (ended.current || lifecycle.current !== epoch) {
        opened.close();
        return;
      }
      player.current = opened;
      soundWanted.current = true;
      setSound(true);
      queue.push({ type: "audio", enabled: true });
    } catch {
      toast.error(t("audioUnavailable"));
    } finally {
      soundOpening.current = false;
    }
  }
  function move(event: { clientX: number; clientY: number }) {
    if (
      !frame ||
      switching !== null ||
      !active ||
      clipboardLock.current ||
      browsing ||
      document.activeElement !== surface.current
    )
      return false;
    const bounds = (
      frame.video ? canvas.current : picture.current
    )?.getBoundingClientRect();
    const input = bounds
      ? remotePoint(event.clientX, event.clientY, bounds)
      : null;
    if (input) queue.push(input, frame.displayId);
    return input !== null;
  }
  function button(button: number, pressed: boolean) {
    const name = (["left", "middle", "right"] as const)[button];
    if (name)
      queue.push(
        { type: "button", button: name, pressed },
        frameRef.current?.displayId,
      );
  }
  function send(input: RemoteInput) {
    if (active && frame && !clipboardLock.current && !browsing)
      queue.push(input, frame.displayId);
  }
  async function clipboard(direction: "toHost" | "fromHost", paste = false) {
    if (
      !active ||
      clipboardLock.current ||
      browsing ||
      switching !== null ||
      ended.current
    )
      return;
    clipboardLock.current = true;
    setClipboardBusy(true);
    release();
    try {
      if (!(await queue.drain()) || ended.current) return;
      const status = await source.clipboard(session.id, direction, paste);
      if (ended.current) return;
      if (status === "completed")
        toast.success(
          t(direction === "toHost" ? "clipboard.sent" : "clipboard.received"),
        );
      else toast.error(t(`clipboard.${status}`));
    } catch (error) {
      if (!ended.current) toast.error(remoteError(error));
    } finally {
      clipboardLock.current = false;
      setClipboardBusy(false);
    }
  }
  const point =
    frame && cursor?.displayId === frame.displayId
      ? cursor.position
      : frame?.cursor;
  return (
    <div
      className="flex min-w-0 flex-col gap-3"
      data-remote-fullscreen={full || undefined}
      data-remote-scaling={scaling}
      data-controls-hidden={(full && !controls) || undefined}
      onPointerMove={(event) => {
        if (full && event.clientY < 64) revealControls();
      }}
    >
      <div
        ref={toolbar}
        data-remote-toolbar
        onFocusCapture={revealControls}
        className="flex min-w-0 flex-wrap items-center gap-2"
      >
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
        <ToggleGroup
          type="single"
          variant="outline"
          size="sm"
          value={quality}
          onValueChange={(value) => {
            if (!value) return;
            release();
            setQualityPending(value as RemoteQuality);
            queue.push({ type: "quality", quality: value as RemoteQuality });
          }}
          disabled={!active || clipboardBusy || qualityPending !== null}
          aria-label={t("quality.label")}
        >
          {(["auto", "smooth", "balanced", "sharp"] as const).map((value) => (
            <ToggleGroupItem key={value} value={value}>
              {t(`quality.${value}`)}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
        <ToggleGroup
          type="single"
          variant="outline"
          size="sm"
          value={scaling}
          onValueChange={(value) => {
            if (!value) return;
            release();
            setScaling(value as "fit" | "actual");
            if (value === "fit") setBrowsing(false);
          }}
          aria-label={t("scaling.label")}
        >
          <ToggleGroupItem value="fit">{t("scaling.fit")}</ToggleGroupItem>
          <ToggleGroupItem value="actual">1:1</ToggleGroupItem>
        </ToggleGroup>
        {scaling === "actual" ? (
          <Button
            size="sm"
            variant="outline"
            aria-pressed={browsing}
            onClick={() => {
              release();
              setBrowsing(!browsing);
            }}
          >
            <Hand data-icon="inline-start" />
            {t(browsing ? "scaling.control" : "scaling.browse")}
          </Button>
        ) : null}
        <Button size="sm" variant="outline" onClick={onSendFiles}>
          <Upload data-icon="inline-start" />
          {t("files")}
        </Button>
        <Button
          size="sm"
          variant="outline"
          aria-pressed={sound}
          disabled={!active}
          onClick={() => void toggleSound()}
        >
          {sound ? (
            <Volume2 data-icon="inline-start" />
          ) : (
            <VolumeX data-icon="inline-start" />
          )}
          {t(sound ? "muteAudio" : "enableAudio")}
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={!active || clipboardBusy}
          onClick={() => void clipboard("toHost")}
        >
          <ClipboardPaste data-icon="inline-start" />
          {t("clipboard.send")}
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={!active || clipboardBusy}
          onClick={() => void clipboard("fromHost")}
        >
          <ClipboardCopy data-icon="inline-start" />
          {t("clipboard.receive")}
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
        {active
          ? t(browsing ? "scaling.browseHint" : "keyboardHint")
          : recovering
            ? t("recovering")
            : verification
              ? t("verify", { code: verification })
              : t("securing")}
      </p>
      {/* 远程输入没有对应的原生 HTML 控件；Esc 始终返回本机，避免键盘被困在画面里。 */}
      <div
        ref={surface}
        data-remote-viewport
        role="application"
        tabIndex={active && frame ? 0 : -1}
        aria-label={t("screen")}
        aria-describedby={hintId}
        className={cn(
          "relative min-h-64 min-w-0 overflow-auto rounded-lg border bg-muted focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring",
          full ? "h-[calc(100dvh-12rem)]" : "max-h-[70vh]",
          browsing && "cursor-grab active:cursor-grabbing",
        )}
        onContextMenu={(e) => e.preventDefault()}
        onBlur={release}
        onPointerMove={(e) => {
          const drag = pan.current;
          if (browsing && drag?.id === e.pointerId) {
            e.currentTarget.scrollLeft = drag.left + drag.x - e.clientX;
            e.currentTarget.scrollTop = drag.top + drag.y - e.clientY;
          } else move(e);
        }}
        onPointerDown={(e) => {
          if (!active || !frame || switching !== null || clipboardLock.current)
            return;
          if (browsing) {
            if (e.pointerType === "touch") return;
            e.preventDefault();
            surface.current?.focus();
            if (e.button !== 0) return;
            pan.current = {
              id: e.pointerId,
              x: e.clientX,
              y: e.clientY,
              left: e.currentTarget.scrollLeft,
              top: e.currentTarget.scrollTop,
            };
            e.currentTarget.setPointerCapture(e.pointerId);
            return;
          }
          e.preventDefault();
          surface.current?.focus();
          if (!move(e)) return;
          pointer.current = e.pointerId;
          e.currentTarget.setPointerCapture(e.pointerId);
          button(e.button, true);
        }}
        onPointerUp={(e) => {
          if (pan.current?.id === e.pointerId) {
            pan.current = null;
            if (e.currentTarget.hasPointerCapture(e.pointerId))
              e.currentTarget.releasePointerCapture(e.pointerId);
            return;
          }
          if (!active || pointer.current !== e.pointerId) return;
          move(e);
          button(e.button, false);
          pointer.current = null;
          if (e.currentTarget.hasPointerCapture(e.pointerId))
            e.currentTarget.releasePointerCapture(e.pointerId);
        }}
        onPointerCancel={() => {
          pan.current = null;
          release();
        }}
        onLostPointerCapture={() => {
          if (pointer.current !== null) release();
        }}
        onKeyDown={(e) => {
          if (!active) return;
          if (e.key === "Escape") {
            e.preventDefault();
            release();
            if (browsing) {
              setBrowsing(false);
              return;
            }
            e.currentTarget.blur();
            if (full) void fullscreen();
            return;
          }
          if (browsing) return;
          if (e.nativeEvent.isComposing) return;
          if (clipboardShortcut(e)) {
            e.preventDefault();
            if (!e.repeat) void clipboard("toHost", true);
            return;
          }
          if (clipboardLock.current) {
            e.preventDefault();
            return;
          }
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
        {frame?.video ? (
          <canvas
            ref={canvas}
            width={frame.width}
            height={frame.height}
            aria-label={t("screen")}
            className={cn(
              "m-auto block select-none",
              scaling === "fit" ? "max-h-[70vh] max-w-full" : "max-w-none",
            )}
          />
        ) : frame ? (
          <img
            ref={picture}
            src={`data:image/jpeg;base64,${frame.jpeg}`}
            alt={t("screen")}
            width={frame.width}
            height={frame.height}
            draggable={false}
            className={cn(
              "m-auto block select-none object-contain",
              scaling === "fit" ? "max-h-[70vh] max-w-full" : "max-w-none",
            )}
            onError={() => finish(t("failed"))}
          />
        ) : (
          <p className="p-6 text-sm text-muted-foreground">
            {active ? t("waitingFrame") : t("pending")}
          </p>
        )}
        {point && pictureBox.width > 0 ? (
          <Circle
            aria-hidden="true"
            className="pointer-events-none absolute size-3 -translate-x-1/2 -translate-y-1/2 fill-background stroke-foreground"
            style={{
              left: pictureBox.left + (point.x / 65535) * pictureBox.width,
              top: pictureBox.top + (point.y / 65535) * pictureBox.height,
            }}
          />
        ) : null}
      </div>
      <form
        data-remote-text
        onFocusCapture={revealControls}
        onSubmit={(e) => {
          e.preventDefault();
          if (!text.trim() || !active || !frame || clipboardBusy) return;
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
                disabled={!active || !frame || clipboardBusy}
              />
              <Button
                type="submit"
                size="sm"
                variant="outline"
                disabled={!active || !frame || clipboardBusy || !text.trim()}
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
