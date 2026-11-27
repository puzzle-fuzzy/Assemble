// 唯一线路契约对应 assemble-protocol/src/preview.rs；不接受凭据或通用 URL。
export interface FilePreview {
  kind: "image" | "text" | "audio" | "video" | "unsupported";
  mimeType: string | null;
  size: number;
  text: string | null;
  truncated: boolean;
}

export interface PreviewResource {
  preview: FilePreview;
  src: string | null;
  dispose?: () => void;
}

const mimeTypes = {
  image: ["image/jpeg", "image/png", "image/webp"],
  text: ["text/plain; charset=utf-8"],
  audio: ["audio/mpeg", "audio/wav", "audio/mp4", "audio/webm"],
  video: ["video/mp4", "video/webm"],
  unsupported: [],
};

export function isFilePreview(value: unknown): value is FilePreview {
  if (!value || typeof value !== "object") return false;
  const v = value as Record<string, unknown>;
  if (
    Object.keys(v).some(
      (key) => !["kind", "mimeType", "size", "text", "truncated"].includes(key),
    )
  )
    return false;
  if (
    typeof v.kind !== "string" ||
    !Object.hasOwn(mimeTypes, v.kind) ||
    typeof v.size !== "number" ||
    !Number.isSafeInteger(v.size) ||
    v.size < 0 ||
    typeof v.truncated !== "boolean"
  )
    return false;
  const kind = v.kind as FilePreview["kind"];
  if (
    kind === "unsupported"
      ? v.mimeType !== null
      : typeof v.mimeType !== "string" ||
        !(mimeTypes[kind] as string[]).includes(v.mimeType)
  )
    return false;
  return kind === "text"
    ? typeof v.text === "string" &&
        new TextEncoder().encode(v.text).length <= 1024 * 1024
    : v.text === null && v.truncated === false;
}
