// 远程响应的独立契约；凭据与本机路径永不经过这里。
export const remoteEndReasons = [
  "stopped",
  "declined",
  "expired",
  "disconnected",
  "deviceRevoked",
  "captureFailed",
  "inputFailed",
] as const;
export type RemoteEndReason = (typeof remoteEndReasons)[number];
export interface RemoteSession {
  id: string;
  controllerId: string;
  controllerName: string;
  hostId: string;
  hostName: string;
  phase: "pending" | "active" | "ended";
  createdAt: string;
  endReason?: RemoteEndReason;
  verification?: string;
}
export interface RemoteView {
  sessions: RemoteSession[];
}
export interface RemoteDisplay {
  id: number;
  name: string;
  width: number;
  height: number;
}
export interface RemoteFrame {
  quality?: RemoteQuality;
  requestedQuality?: RemoteQuality;
  video?: RemoteVideo;
  cursor?: RemoteCursor;
  sequence: number;
  displayId: number;
  width: number;
  height: number;
  jpeg: string;
  displays: RemoteDisplay[];
}
export type RemoteQuality = "auto" | "smooth" | "balanced" | "sharp";
export interface RemoteVideo {
  stream: number;
  codec: string;
  keyframe: boolean;
  timestamp: number;
  data: string;
}
export interface RemoteCursor {
  x: number;
  y: number;
}
export interface CursorUpdate {
  displayId: number;
  position: RemoteCursor | null;
}
export type RemoteInput =
  | { type: "move"; x: number; y: number }
  | { type: "button"; button: "left" | "right" | "middle"; pressed: boolean }
  | { type: "key"; key: string; pressed: boolean }
  | { type: "scroll"; x: number; y: number }
  | { type: "text"; text: string }
  | { type: "display"; id: number }
  | { type: "quality"; quality: RemoteQuality }
  | { type: "video"; enabled: boolean }
  | { type: "keyframe" }
  | { type: "audio"; enabled: boolean }
  | { type: "releaseAll" };
export interface SequencedInput {
  sequence: number;
  displayId?: number;
  input: RemoteInput;
}
export interface RemotePoll {
  session: RemoteSession;
  frame: RemoteFrame | null;
  inputs: SequencedInput[];
  frames?: RemoteFrame[];
  cursor?: CursorUpdate | null;
}
export interface RemoteBridge {
  audio(id: string, after: number): Promise<AudioPlayback>;
  clipboard(
    id: string,
    direction: "toHost" | "fromHost",
    paste: boolean,
  ): Promise<ClipboardStatus>;
  fullscreen(enabled: boolean): Promise<void>;
  request(hostId: string): Promise<RemoteSession>;
  approve(id: string): Promise<RemoteSession>;
  end(id: string): Promise<void>;
  poll(id: string, frameAfter: number): Promise<RemotePoll>;
  inputs(id: string, events: SequencedInput[]): Promise<void>;
}
export interface AudioPlayback {
  state: "off" | "active" | "unavailable";
  packets: { sequence: number; samples: number[] }[];
}
export function isAudioPlayback(value: unknown): value is AudioPlayback {
  return (
    object(value) &&
    ["off", "active", "unavailable"].includes(String(value.state)) &&
    Array.isArray(value.packets) &&
    value.packets.length <= 24 &&
    value.packets.every(
      (packet) =>
        object(packet) &&
        whole(packet.sequence) &&
        packet.sequence > 0 &&
        Array.isArray(packet.samples) &&
        packet.samples.length === 1920 &&
        packet.samples.every(
          (sample) => typeof sample === "number" && Number.isFinite(sample),
        ),
    )
  );
}
export type ClipboardStatus =
  | "completed"
  | "unavailable"
  | "tooLarge"
  | "expired";
export const isClipboardStatus = (value: unknown): value is ClipboardStatus =>
  ["completed", "unavailable", "tooLarge", "expired"].some(
    (status) => status === value,
  );
const object = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const whole = (v: unknown, max = 0xffff_ffff): v is number =>
  typeof v === "number" && Number.isSafeInteger(v) && v >= 0 && v <= max;
export function isRemoteSession(v: unknown): v is RemoteSession {
  return (
    object(v) &&
    ["id", "controllerId", "controllerName", "hostId", "hostName"].every(
      (k) =>
        typeof v[k] === "string" &&
        (v[k] as string).length > 0 &&
        (v[k] as string).length <= 256,
    ) &&
    ["pending", "active", "ended"].includes(String(v.phase)) &&
    typeof v.createdAt === "string" &&
    Number.isFinite(Date.parse(v.createdAt)) &&
    (v.verification === undefined ||
      (typeof v.verification === "string" &&
        /^(?:[a-f0-9]{4}-){5}[a-f0-9]{4}$/.test(v.verification))) &&
    (v.endReason === undefined ||
      (v.phase === "ended" &&
        remoteEndReasons.some((reason) => reason === v.endReason)))
  );
}
export function isRemoteView(v: unknown): v is RemoteView {
  return (
    object(v) &&
    Array.isArray(v.sessions) &&
    v.sessions.length <= 128 &&
    v.sessions.every(isRemoteSession)
  );
}
export function isRemoteFrame(v: unknown): v is RemoteFrame {
  return (
    object(v) &&
    whole(v.sequence) &&
    v.sequence > 0 &&
    whole(v.displayId) &&
    whole(v.width, 1920) &&
    v.width > 0 &&
    whole(v.height, 1920) &&
    v.height > 0 &&
    (v.cursor === undefined || isRemoteCursor(v.cursor)) &&
    (v.requestedQuality === undefined ||
      ["auto", "smooth", "balanced", "sharp"].includes(
        String(v.requestedQuality),
      )) &&
    (v.quality === undefined ||
      ["smooth", "balanced", "sharp"].includes(String(v.quality))) &&
    typeof v.jpeg === "string" &&
    ((v.video === undefined &&
      v.jpeg.length > 0 &&
      v.jpeg.length <= 699052 &&
      /^[A-Za-z0-9+/]+={0,2}$/.test(v.jpeg)) ||
      (v.jpeg === "" && isRemoteVideo(v.video))) &&
    Array.isArray(v.displays) &&
    v.displays.length > 0 &&
    v.displays.length <= 16 &&
    v.displays.every(
      (d) =>
        object(d) &&
        whole(d.id) &&
        typeof d.name === "string" &&
        d.name.length <= 256 &&
        whole(d.width, 32768) &&
        d.width > 0 &&
        whole(d.height, 32768) &&
        d.height > 0,
    ) &&
    v.displays.some((d) => d.id === v.displayId)
  );
}
export function isRemotePoll(v: unknown): v is RemotePoll {
  return (
    object(v) &&
    isRemoteSession(v.session) &&
    (v.cursor === undefined ||
      v.cursor === null ||
      (object(v.cursor) &&
        whole(v.cursor.displayId) &&
        (v.cursor.position === null || isRemoteCursor(v.cursor.position)))) &&
    (v.frame === null || isRemoteFrame(v.frame)) &&
    Array.isArray(v.inputs) &&
    v.inputs.length === 0 &&
    (v.clipboard === undefined || v.clipboard === null) &&
    (v.frames === undefined ||
      (Array.isArray(v.frames) &&
        v.frames.length <= 8 &&
        v.frames.every(isRemoteFrame)))
  );
}
function isRemoteCursor(value: unknown): value is RemoteCursor {
  return object(value) && whole(value.x, 65535) && whole(value.y, 65535);
}
export function isRemoteVideo(value: unknown): value is RemoteVideo {
  return (
    object(value) &&
    whole(value.stream) &&
    value.stream > 0 &&
    typeof value.codec === "string" &&
    /^avc1\.[0-9a-fA-F]{6}$/.test(value.codec) &&
    typeof value.keyframe === "boolean" &&
    whole(value.timestamp, Number.MAX_SAFE_INTEGER) &&
    typeof value.data === "string" &&
    value.data.length > 0 &&
    value.data.length <= 699052 &&
    /^[A-Za-z0-9+/]+={0,2}$/.test(value.data)
  );
}
