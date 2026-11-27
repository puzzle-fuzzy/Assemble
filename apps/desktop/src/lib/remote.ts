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
  sequence: number;
  displayId: number;
  width: number;
  height: number;
  jpeg: string;
  displays: RemoteDisplay[];
}
export type RemoteInput =
  | { type: "move"; x: number; y: number }
  | { type: "button"; button: "left" | "right" | "middle"; pressed: boolean }
  | { type: "key"; key: string; pressed: boolean }
  | { type: "scroll"; x: number; y: number }
  | { type: "text"; text: string }
  | { type: "display"; id: number }
  | { type: "releaseAll" };
export interface SequencedInput {
  sequence: number;
  input: RemoteInput;
}
export interface RemotePoll {
  session: RemoteSession;
  frame: RemoteFrame | null;
  inputs: SequencedInput[];
}
export interface RemoteBridge {
  fullscreen(enabled: boolean): Promise<void>;
  request(hostId: string): Promise<RemoteSession>;
  approve(id: string): Promise<RemoteSession>;
  end(id: string): Promise<void>;
  poll(id: string, frameAfter: number): Promise<RemotePoll>;
  inputs(id: string, events: SequencedInput[]): Promise<void>;
}
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
    typeof v.jpeg === "string" &&
    v.jpeg.length > 0 &&
    v.jpeg.length <= 699052 &&
    /^[A-Za-z0-9+/]+={0,2}$/.test(v.jpeg) &&
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
    (v.frame === null || isRemoteFrame(v.frame)) &&
    Array.isArray(v.inputs) &&
    v.inputs.length === 0
  );
}
