export const platforms = ["windows", "macos", "ios", "android"] as const;
export type Platform = (typeof platforms)[number];
export type Architecture = "x64" | "arm64" | "universal";
export interface Artifact {
  platform: Platform;
  architecture: Architecture;
  version: string;
  fileName: string;
  sha256: string;
  size: number;
  publishedAt: string;
}
export interface Manifest {
  schemaVersion: 1;
  artifacts: Artifact[];
}
export const MAX_MANIFEST_BYTES = 128 * 1024;
export const MAX_ARTIFACT_BYTES = 2 * 1024 ** 3;
const fields = [
  "platform",
  "architecture",
  "version",
  "fileName",
  "sha256",
  "size",
  "publishedAt",
];
export function validReleaseTime(value: unknown): value is string {
  if (
    typeof value !== "string" ||
    !/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(value)
  )
    return false;
  const date = new Date(value);
  return Number.isFinite(date.getTime()) && date.toISOString() === value;
}
export function validArtifact(value: unknown): value is Artifact {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const item = value as Record<string, unknown>;
  if (
    Object.keys(item).length !== fields.length ||
    !fields.every((key) => key in item)
  )
    return false;
  if (
    !platforms.includes(item.platform as Platform) ||
    !["x64", "arm64", "universal"].includes(String(item.architecture))
  )
    return false;
  if (item.platform === "ios" && item.architecture !== "arm64") return false;
  if (item.platform === "windows" && item.architecture === "universal")
    return false;
  const extension =
    typeof item.fileName === "string"
      ? item.fileName.split(".").at(-1)?.toLowerCase()
      : "";
  const formats = {
    windows: ["exe", "msi"],
    macos: ["dmg", "pkg"],
    ios: ["ipa"],
    android: ["apk"],
  };
  return (
    typeof item.version === "string" &&
    /^[0-9][0-9A-Za-z.+-]{0,63}$/.test(item.version) &&
    typeof item.fileName === "string" &&
    /^Assemble-[A-Za-z0-9._-]+$/.test(item.fileName) &&
    item.fileName.length <= 128 &&
    !item.fileName.includes("..") &&
    formats[item.platform as Platform].includes(extension ?? "") &&
    typeof item.sha256 === "string" &&
    /^[0-9a-f]{64}$/.test(item.sha256) &&
    typeof item.size === "number" &&
    Number.isSafeInteger(item.size) &&
    item.size > 0 &&
    item.size <= MAX_ARTIFACT_BYTES &&
    validReleaseTime(item.publishedAt)
  );
}
export function parseManifest(value: unknown): Manifest {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("INVALID_RELEASE_MANIFEST");
  const item = value as Record<string, unknown>;
  if (
    Object.keys(item).length !== 2 ||
    item.schemaVersion !== 1 ||
    !Array.isArray(item.artifacts) ||
    item.artifacts.length > 128 ||
    !item.artifacts.every(validArtifact)
  )
    throw new Error("INVALID_RELEASE_MANIFEST");
  const ids = new Set<string>();
  const versions = new Set<string>();
  for (const artifact of item.artifacts) {
    const key = `${artifact.platform}:${artifact.architecture}:${artifact.version}`;
    if (ids.has(artifact.sha256) || versions.has(key))
      throw new Error("INVALID_RELEASE_MANIFEST");
    ids.add(artifact.sha256);
    versions.add(key);
  }
  return item as unknown as Manifest;
}
export function downloadPath(artifact: Artifact): string {
  if (!validArtifact(artifact)) throw new Error("INVALID_RELEASE_ARTIFACT");
  return `/v1/releases/files/${artifact.sha256}`;
}
