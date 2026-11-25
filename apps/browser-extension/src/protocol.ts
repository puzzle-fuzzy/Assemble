export const NATIVE_HOST = "com.puzzlefuzzy.assemble";
export type SourceKind = "hls" | "douyin" | "video";
export interface FoundSource {
  url: string;
  kind: SourceKind;
  title: string;
  bytes?: number;
}
export interface SourceItem {
  id: string;
  kind: SourceKind;
  title: string;
  host: string;
  bytes: number | null;
  submitted: boolean;
}
export interface TabSources {
  pageUrl: string;
  title: string;
  updatedAt: number;
  capturing: boolean;
  sources: (FoundSource & {
    id: string;
    requestId: string;
    submitted: boolean;
  })[];
}
export interface SourceList {
  items: SourceItem[];
  capturing: boolean;
}
export interface NativeReply {
  version: 1;
  ok: boolean;
  taskId: string | null;
  errorCode: string | null;
}
export type PopupRequest =
  | { type: "scan" | "list" | "status" }
  | { type: "capture"; enabled: boolean }
  | { type: "download"; id: string };
export type PopupReply =
  | { ok: true; list?: SourceList }
  | { ok: false; code: string };

export function publicUrl(value: string, base?: string): URL | null {
  try {
    const url = new URL(value, base);
    if (
      !["http:", "https:"].includes(url.protocol) ||
      url.username ||
      url.password ||
      value.length > 16384
    )
      return null;
    url.hash = "";
    return url;
  } catch {
    return null;
  }
}
export function sourceKind(value: string, mime = ""): SourceKind | null {
  const url = publicUrl(value);
  if (!url) return null;
  if (
    ["douyin.com", "iesdouyin.com"].some(
      (host) => url.hostname === host || url.hostname.endsWith(`.${host}`),
    ) &&
    (/\/(?:video|share\/video)\/\d{10,25}(?:\/|$)/.test(url.pathname) ||
      url.searchParams.has("modal_id") ||
      url.hostname === "v.douyin.com")
  )
    return "douyin";
  if (
    /\.m3u8$/i.test(url.pathname) ||
    /^(?:application|audio)\/(?:vnd\.apple\.mpegurl|x-mpegurl|mpegurl)$/i.test(
      mime.split(";", 1)[0]?.trim() ?? "",
    )
  )
    return "hls";
  if (
    /\.(?:mp4|m4v|webm|mov)$/i.test(url.pathname) ||
    /^video\/(?:mp4|webm|quicktime)(?:;|$)/i.test(mime)
  )
    return "video";
  return null;
}
export function sourceView(state: TabSources): SourceList {
  return {
    capturing: state.capturing,
    items: state.sources.map((source) => ({
      id: source.id,
      kind: source.kind,
      title: source.title,
      host: publicUrl(source.url)?.hostname ?? "",
      bytes: source.bytes ?? null,
      submitted: source.submitted,
    })),
  };
}
export function isNativeReply(value: unknown): value is NativeReply {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    v.version === 1 &&
    typeof v.ok === "boolean" &&
    (v.taskId === null || typeof v.taskId === "string") &&
    (v.errorCode === null || typeof v.errorCode === "string") &&
    Object.keys(v).every((key) =>
      ["version", "ok", "taskId", "errorCode"].includes(key),
    )
  );
}
