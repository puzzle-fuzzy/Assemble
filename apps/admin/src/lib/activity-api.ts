import type { AdminPage } from "./api";
export type ActivityKind =
  | "file"
  | "transfer"
  | "media"
  | "collection"
  | "remote";
export type ActivityCategory =
  | "all"
  | "files"
  | "transfers"
  | "media"
  | "remote";
export type ActivitySnapshot =
  | { kind: "file"; name: string; size: number; mimeType: string | null }
  | {
      kind: "transfer";
      name: string;
      size: number;
      fromDeviceId: string;
      fromDeviceName: string;
      toDeviceId: string;
      toDeviceName: string;
      state: string;
      failure: string | null;
    }
  | { kind: "media"; title: string; scope: "shared" | "private" }
  | { kind: "collection"; title: string }
  | {
      kind: "remote";
      controllerId: string;
      controllerName: string;
      hostId: string;
      hostName: string;
      phase: string;
      endReason: string | null;
    };
export interface ActivitySubject {
  kind: ActivityKind;
  id: string;
  name: string | null;
  ownerId: string | null;
  available: boolean;
}
export interface AdminActivity {
  id: string;
  actor: {
    accountId: string | null;
    name: string | null;
    deviceId: string | null;
    deviceName: string | null;
  };
  action: string;
  category: ActivityCategory;
  occurredAt: string;
  subject: ActivitySubject | null;
  snapshot: ActivitySnapshot | null;
}
export interface AdminActivityTarget {
  subject: ActivitySubject;
  snapshot: ActivitySnapshot | null;
  createdAt: string;
  updatedAt: string;
  events: AdminActivity[];
}
export type ActivityPageData = AdminPage<AdminActivity>;
export interface ActivityTargetRef {
  kind: ActivityKind;
  id: string;
}
const kinds: ActivityKind[] = [
  "file",
  "transfer",
  "media",
  "collection",
  "remote",
];
export function readActivityTarget(location: string): ActivityTargetRef | null {
  const [page, kind, id, ...extra] = location.split("?")[0]?.split("/") ?? [];
  if (
    page !== "activity" ||
    !kinds.includes(kind as ActivityKind) ||
    !id ||
    extra.length
  )
    return null;
  try {
    return { kind: kind as ActivityKind, id: decodeURIComponent(id) };
  } catch {
    return null;
  }
}
export function subjectHref(subject: ActivitySubject): string {
  const id = encodeURIComponent(subject.id);
  if (subject.available && subject.kind === "file") return `#drive?file=${id}`;
  if (subject.available && subject.kind === "media")
    return `#media?entry=${id}`;
  if (subject.available && subject.kind === "collection")
    return `#media?collection=${id}`;
  return `#activity/${subject.kind}/${id}`;
}
export function activityPath(filters: {
  q: string;
  category: ActivityCategory;
  from: string;
  through: string;
  page: number;
  pageSize: number;
}): string {
  const query = new URLSearchParams({
    q: filters.q,
    category: filters.category,
    from: filters.from,
    through: filters.through,
    page: String(filters.page),
    pageSize: String(filters.pageSize),
  });
  return `/v1/admin/activity?${query}`;
}
