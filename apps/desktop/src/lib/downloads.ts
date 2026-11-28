// 本机下载的公开展示契约；地址只在提交时单向交给 Rust。
export type DownloadKind = "hls" | "douyin" | "video" | "magnet";
export type MagnetNetwork = "system" | "direct";
export interface DownloadInput {
  magnetNetwork?: MagnetNetwork;
  kind: DownloadKind;
  url: string;
  pageUrl?: string | null;
  title?: string | null;
}
export interface DownloadVariant {
  id: string;
  width: number | null;
  height: number | null;
  bandwidth: number | null;
}
export interface DownloadFile {
  id: number;
  name: string;
  size: number;
}
export interface DownloadPreview {
  magnetNetwork?: MagnetNetwork;
  id: string;
  kind: DownloadKind;
  title: string;
  sourceHost: string;
  durationSeconds: number | null;
  variants: DownloadVariant[];
  files?: DownloadFile[];
}
export type DownloadPhase =
  | "queued"
  | "downloading"
  | "receiving"
  | "muxing"
  | "verifying"
  | "completed"
  | "failed"
  | "cancelled"
  | "interrupted"
  | "paused";
export interface DownloadTask {
  magnetNetwork?: MagnetNetwork;
  /** 旧任务缺省；来源标记不包含助手身份或来源凭据。 */
  initiatedBy?: "assistant";
  id: string;
  kind: DownloadKind;
  title: string;
  fileName: string | null;
  sourceHost: string;
  phase: DownloadPhase;
  receivedBytes: number;
  totalBytes: number | null;
  completedSegments: number;
  totalSegments: number | null;
  bytesPerSecond: number;
  mediaSeconds: number | null;
  durationSeconds: number | null;
  errorCode: string | null;
  retryable: boolean;
  createdAt: string;
}
export interface DownloadsView {
  directBtAvailable?: boolean;
  assistantAllowed: boolean;
  /** 旧开发预览缺少此定位信号时按未触发处理。 */
  browserTaskId?: string | null;
  folderName: string;
  toolsAvailable: boolean;
  tasks: DownloadTask[];
}

const record = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const positive = (value: unknown) =>
  typeof value === "number" && Number.isFinite(value) && value >= 0;
const optionalNumber = (value: unknown) => value === null || positive(value);
const optionalString = (value: unknown) =>
  value === null || typeof value === "string";
const kind = (value: unknown) =>
  value === "hls" ||
  value === "douyin" ||
  value === "video" ||
  value === "magnet";

const validNetwork = (value: Record<string, unknown>) =>
  value.magnetNetwork === undefined ||
  (value.kind === "magnet" &&
    (value.magnetNetwork === "system" || value.magnetNetwork === "direct"));
export function isDownloadPreview(value: unknown): value is DownloadPreview {
  return (
    record(value) &&
    validNetwork(value) &&
    typeof value.id === "string" &&
    kind(value.kind) &&
    typeof value.title === "string" &&
    typeof value.sourceHost === "string" &&
    optionalNumber(value.durationSeconds) &&
    Array.isArray(value.variants) &&
    value.variants.length > 0 &&
    (value.files === undefined ||
      (Array.isArray(value.files) &&
        value.files.length <= 10000 &&
        value.files.every(
          (f) =>
            record(f) &&
            Number.isInteger(f.id) &&
            Number(f.id) >= 0 &&
            Number(f.id) <= 0xffffffff &&
            typeof f.name === "string" &&
            f.name.length > 0 &&
            f.name.length <= 2048 &&
            Number.isSafeInteger(f.size) &&
            Number(f.size) >= 0,
        ) &&
        new Set(value.files.map((f) => f.id)).size === value.files.length)) &&
    (value.kind !== "magnet" ||
      (Array.isArray(value.files) && value.files.length > 0)) &&
    value.variants.every(
      (v) =>
        record(v) &&
        typeof v.id === "string" &&
        optionalNumber(v.width) &&
        optionalNumber(v.height) &&
        optionalNumber(v.bandwidth),
    )
  );
}
export function isDownloadsView(value: unknown): value is DownloadsView {
  return (
    record(value) &&
    (value.directBtAvailable === undefined ||
      typeof value.directBtAvailable === "boolean") &&
    typeof value.folderName === "string" &&
    typeof value.assistantAllowed === "boolean" &&
    (value.browserTaskId === undefined ||
      optionalString(value.browserTaskId)) &&
    typeof value.toolsAvailable === "boolean" &&
    Array.isArray(value.tasks) &&
    value.tasks.every(
      (v) =>
        record(v) &&
        validNetwork(v) &&
        typeof v.id === "string" &&
        (v.initiatedBy === undefined || v.initiatedBy === "assistant") &&
        kind(v.kind) &&
        typeof v.title === "string" &&
        typeof v.sourceHost === "string" &&
        typeof v.createdAt === "string" &&
        typeof v.phase === "string" &&
        [
          "queued",
          "downloading",
          "receiving",
          "muxing",
          "verifying",
          "completed",
          "failed",
          "cancelled",
          "interrupted",
          "paused",
        ].includes(v.phase) &&
        optionalString(v.fileName) &&
        optionalString(v.errorCode) &&
        typeof v.retryable === "boolean" &&
        positive(v.receivedBytes) &&
        optionalNumber(v.totalBytes) &&
        positive(v.completedSegments) &&
        optionalNumber(v.totalSegments) &&
        positive(v.bytesPerSecond) &&
        optionalNumber(v.mediaSeconds) &&
        optionalNumber(v.durationSeconds),
    )
  );
}
export function downloadProgress(task: DownloadTask): number | null {
  const ratio =
    task.phase === "muxing"
      ? task.durationSeconds && task.mediaSeconds !== null
        ? task.mediaSeconds / task.durationSeconds
        : null
      : ["downloading", "receiving", "paused"].includes(task.phase)
        ? task.totalSegments
          ? task.completedSegments / task.totalSegments
          : task.totalBytes
            ? task.receivedBytes / task.totalBytes
            : null
        : null;
  return ratio === null ? null : Math.min(100, Math.max(0, ratio * 100));
}
