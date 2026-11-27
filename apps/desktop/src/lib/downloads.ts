// 本机下载的公开展示契约；地址只在提交时单向交给 Rust。
export type DownloadKind = "hls" | "douyin" | "video";
export interface DownloadInput {
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
export interface DownloadPreview {
  id: string;
  kind: DownloadKind;
  title: string;
  sourceHost: string;
  durationSeconds: number | null;
  variants: DownloadVariant[];
}
export type DownloadPhase =
  | "queued"
  | "downloading"
  | "muxing"
  | "verifying"
  | "completed"
  | "failed"
  | "cancelled"
  | "interrupted";
export interface DownloadTask {
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
  value === "hls" || value === "douyin" || value === "video";

export function isDownloadPreview(value: unknown): value is DownloadPreview {
  return (
    record(value) &&
    typeof value.id === "string" &&
    kind(value.kind) &&
    typeof value.title === "string" &&
    typeof value.sourceHost === "string" &&
    optionalNumber(value.durationSeconds) &&
    Array.isArray(value.variants) &&
    value.variants.length > 0 &&
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
    typeof value.folderName === "string" &&
    typeof value.assistantAllowed === "boolean" &&
    (value.browserTaskId === undefined ||
      optionalString(value.browserTaskId)) &&
    typeof value.toolsAvailable === "boolean" &&
    Array.isArray(value.tasks) &&
    value.tasks.every(
      (v) =>
        record(v) &&
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
          "muxing",
          "verifying",
          "completed",
          "failed",
          "cancelled",
          "interrupted",
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
      : task.phase === "downloading"
        ? task.totalSegments
          ? task.completedSegments / task.totalSegments
          : task.totalBytes
            ? task.receivedBytes / task.totalBytes
            : null
        : null;
  return ratio === null ? null : Math.min(100, Math.max(0, ratio * 100));
}
