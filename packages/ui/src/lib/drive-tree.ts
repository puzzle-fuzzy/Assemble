/** 与 assemble-protocol::drive_tree 的独立线路类型对应。 */
export interface DriveFolder {
  id: string;
  parentId: string | null;
  name: string;
  version: number;
  createdAt: string;
  updatedAt: string;
}
export interface DriveTreeFile {
  id: string;
  parentId: string | null;
  name: string;
  version: number;
  createdAt: string;
  updatedAt: string | null;
  size: number;
  sha256: string;
  mimeType?: string;
}
export type DriveEntry =
  | ({ kind: "folder" } & DriveFolder)
  | ({ kind: "file" } & DriveTreeFile);
export type DriveKind = DriveEntry["kind"];
export interface DriveTreeQuery {
  parentId: string;
  q: string;
  page: number;
  pageSize: number;
  foldersOnly?: boolean;
}
export interface DriveTreeListing {
  current: DriveFolder | null;
  ancestors: DriveFolder[];
  items: DriveEntry[];
  total: number;
  page: number;
  pageSize: number;
  usedBytes: number;
  limitBytes: number;
}
export interface DriveTreeSource {
  list(
    query: DriveTreeQuery,
    signal?: AbortSignal,
    remember?: boolean,
  ): Promise<DriveTreeListing>;
  create?(input: {
    parentId: string;
    name: string;
    requestId: string;
  }): Promise<DriveFolder>;
  rename?(
    kind: DriveKind,
    id: string,
    input: { name: string; version: number },
  ): Promise<DriveEntry>;
  move?(input: {
    kind: DriveKind;
    id: string;
    parentId: string;
    version: number;
  }): Promise<DriveEntry>;
  remove?(kind: DriveKind, id: string, version: number): Promise<void>;
  upload?(parentId: string): Promise<unknown>;
}
const record = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const id = (value: unknown): value is string =>
  typeof value === "string" && /^[\w.-]{1,200}$/u.test(value);
const time = (value: unknown): value is string =>
  typeof value === "string" &&
  /^\d{4}-\d{2}-\d{2}T/u.test(value) &&
  Number.isFinite(Date.parse(value));
const whole = (value: unknown): value is number =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
function common(value: unknown): value is Record<string, unknown> {
  return (
    record(value) &&
    id(value.id) &&
    (value.parentId === null || id(value.parentId)) &&
    typeof value.name === "string" &&
    whole(value.version) &&
    value.version > 0 &&
    time(value.createdAt)
  );
}
export function isDriveFolder(value: unknown): value is DriveFolder {
  return common(value) && time(value.updatedAt);
}
export function isDriveEntry(value: unknown): value is DriveEntry {
  return (
    common(value) &&
    (value.kind === "folder"
      ? isDriveFolder(value)
      : value.kind === "file" &&
        whole(value.size) &&
        typeof value.sha256 === "string" &&
        /^[a-f0-9]{64}$/u.test(value.sha256) &&
        (value.mimeType === undefined || typeof value.mimeType === "string") &&
        (value.updatedAt === null || time(value.updatedAt)))
  );
}
export function isDriveTreeListing(value: unknown): value is DriveTreeListing {
  return (
    record(value) &&
    (value.current === null || isDriveFolder(value.current)) &&
    Array.isArray(value.ancestors) &&
    value.ancestors.length <= 16 &&
    value.ancestors.every(isDriveFolder) &&
    Array.isArray(value.items) &&
    value.items.length <= 100 &&
    value.items.every(isDriveEntry) &&
    whole(value.total) &&
    whole(value.page) &&
    value.page > 0 &&
    whole(value.pageSize) &&
    value.pageSize >= 1 &&
    value.pageSize <= 100 &&
    whole(value.usedBytes) &&
    whole(value.limitBytes)
  );
}
