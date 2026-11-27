import type { DriveKind, DriveTreeQuery } from "./drive-tree";

export interface TrashTarget {
  kind: DriveKind;
  id: string;
  version: number;
}
export interface TrashPreview {
  target: TrashTarget;
  name: string;
  fileCount: number;
  folderCount: number;
  size: number;
  revision: string;
}
export interface TrashBatch {
  id: string;
  rootId: string;
  kind: DriveKind;
  name: string;
  originalLocation: string[];
  deletedAt: string;
  version: number;
  fileCount: number;
  folderCount: number;
  size: number;
}
export interface TrashListing {
  items: TrashBatch[];
  total: number;
  page: number;
  pageSize: number;
}
export interface TrashItem {
  id: string;
  parentId: string | null;
  kind: DriveKind;
  name: string;
  size: number | null;
  version: number;
  createdAt: string;
}
export interface TrashDetail {
  batch: TrashBatch;
  current: TrashItem;
  ancestors: TrashItem[];
  items: TrashItem[];
  total: number;
  page: number;
  pageSize: number;
}
export interface TrashRestored {
  batchId: string;
  id: string;
  kind: DriveKind;
  name: string;
  parentId: string | null;
  reason: "original" | "parentUnavailable" | "depthLimit";
  renamed: boolean;
}
export interface TrashRestorePreview extends TrashRestored {
  version: number;
  revision: string;
  parentLocation: string[];
}
export interface TrashPurged {
  id: string;
  logicalBytesReleased: number;
  cleanupPending: boolean;
}
export interface TrashSource {
  list(
    q: string,
    page: number,
    pageSize: number,
    signal?: AbortSignal,
  ): Promise<TrashListing>;
  detail(
    id: string,
    query: DriveTreeQuery,
    signal?: AbortSignal,
  ): Promise<TrashDetail>;
  preview?(target: TrashTarget, signal?: AbortSignal): Promise<TrashPreview>;
  move?(input: {
    target: TrashTarget;
    revision: string;
    requestId: string;
  }): Promise<TrashBatch>;
  restorePreview?(
    id: string,
    signal?: AbortSignal,
  ): Promise<TrashRestorePreview>;
  restore?(
    id: string,
    input: { version: number; revision: string },
  ): Promise<TrashRestored>;
  purge?(id: string, version: number): Promise<TrashPurged>;
}
const record = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const id = (v: unknown): v is string =>
  typeof v === "string" && /^[\w.-]{1,200}$/u.test(v);
const number = (v: unknown): v is number =>
  typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
const kind = (v: unknown): v is DriveKind => v === "file" || v === "folder";
const time = (v: unknown): v is string =>
  typeof v === "string" &&
  /^\d{4}-\d{2}-\d{2}T/u.test(v) &&
  Number.isFinite(Date.parse(v));
const strings = (v: unknown): v is string[] =>
  Array.isArray(v) && v.length <= 16 && v.every((x) => typeof x === "string");
const revision = (v: unknown): v is string =>
  typeof v === "string" && /^[a-f0-9]{64}$/u.test(v);
const paging = (v: Record<string, unknown>) =>
  number(v.total) &&
  number(v.page) &&
  v.page > 0 &&
  number(v.pageSize) &&
  v.pageSize >= 1 &&
  v.pageSize <= 100;
export const isTrashTarget = (v: unknown): v is TrashTarget =>
  record(v) && kind(v.kind) && id(v.id) && number(v.version) && v.version > 0;
export const isTrashPreview = (v: unknown): v is TrashPreview =>
  record(v) &&
  isTrashTarget(v.target) &&
  typeof v.name === "string" &&
  number(v.fileCount) &&
  number(v.folderCount) &&
  number(v.size) &&
  revision(v.revision);
export const isTrashBatch = (v: unknown): v is TrashBatch =>
  record(v) &&
  id(v.id) &&
  id(v.rootId) &&
  kind(v.kind) &&
  typeof v.name === "string" &&
  strings(v.originalLocation) &&
  time(v.deletedAt) &&
  number(v.version) &&
  v.version > 0 &&
  number(v.fileCount) &&
  number(v.folderCount) &&
  number(v.size);
export const isTrashListing = (v: unknown): v is TrashListing =>
  record(v) &&
  Array.isArray(v.items) &&
  v.items.length <= 100 &&
  v.items.every(isTrashBatch) &&
  paging(v);
export const isTrashItem = (v: unknown): v is TrashItem =>
  record(v) &&
  isTrashTarget(v) &&
  typeof v.name === "string" &&
  (v.parentId === null || id(v.parentId)) &&
  (v.size === null || number(v.size)) &&
  time(v.createdAt);
export const isTrashDetail = (v: unknown): v is TrashDetail =>
  record(v) &&
  isTrashBatch(v.batch) &&
  isTrashItem(v.current) &&
  Array.isArray(v.ancestors) &&
  v.ancestors.length <= 16 &&
  v.ancestors.every(isTrashItem) &&
  Array.isArray(v.items) &&
  v.items.length <= 100 &&
  v.items.every(isTrashItem) &&
  paging(v);
export const isTrashRestored = (v: unknown): v is TrashRestored =>
  record(v) &&
  id(v.batchId) &&
  id(v.id) &&
  kind(v.kind) &&
  typeof v.name === "string" &&
  (v.parentId === null || id(v.parentId)) &&
  ["original", "parentUnavailable", "depthLimit"].includes(String(v.reason)) &&
  typeof v.renamed === "boolean";
export const isTrashRestorePreview = (v: unknown): v is TrashRestorePreview =>
  isTrashRestored(v) &&
  record(v) &&
  number(v.version) &&
  v.version > 0 &&
  revision(v.revision) &&
  strings(v.parentLocation);
export const isTrashPurged = (v: unknown): v is TrashPurged =>
  record(v) &&
  id(v.id) &&
  number(v.logicalBytesReleased) &&
  typeof v.cleanupPending === "boolean";
