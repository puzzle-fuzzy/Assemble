import { AdminRequestError, type RequestFailure } from "./api";

export const metadataStatuses = [
  "pending",
  "fetching",
  "deferred",
  "verified",
  "not_found",
  "invalid",
  "failed",
] as const;
export type MetadataStatus = (typeof metadataStatuses)[number];
export type DhtMode = "stopped" | "collecting" | "draining";
export interface DhtObservation {
  id: string;
  firstSeen: string;
  lastSeen: string;
  announceCount: number;
  sampleCount: number;
  status: MetadataStatus;
  name: string | null;
  totalSize: number | null;
  fileCount: number | null;
  fetchedAt: string | null;
  attempts: number;
  lastError: string | null;
  magnetUri: string;
}
export interface DhtDetails extends DhtObservation {
  files: { path: string[]; length: number }[];
  pieceLength: number | null;
  private: boolean | null;
  comment: string | null;
  source: string | null;
}
export interface DhtStatus {
  enabled: boolean;
  mode: DhtMode;
  running: boolean;
  lastError: string | null;
  total: number;
  announces: number;
  samples: number;
  lastObservedAt: string | null;
  statuses: Partial<Record<MetadataStatus, number>>;
  queueLimit: number;
  maxRecords: number;
  workers: number;
  metadataBytes: number;
  metadataBudgetBytes: number;
  counters: {
    lookups: number;
    lookupsWithPeers: number;
    fetchAttempts: number;
    verified: number;
    timeouts: number;
    connectionErrors: number;
    protocolErrors: number;
    validationErrors: number;
    persistenceErrors: number;
    droppedEvents: number;
  };
}

export class DhtRequestError extends AdminRequestError {
  constructor(
    kind: RequestFailure,
    readonly code: string,
  ) {
    super(kind);
  }
}

export async function dhtWrite<T>(
  action: "query" | "control",
  body: unknown,
  csrfToken: string,
  signal: AbortSignal,
): Promise<T> {
  const response = await fetch(`/v1/admin/dht/${action}`, {
    method: "POST",
    credentials: "same-origin",
    cache: "no-store",
    redirect: "error",
    signal,
    headers: {
      Accept: "application/json",
      "Content-Type": "application/json",
      "X-CSRF-Token": csrfToken,
    },
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    let code = "unavailable";
    try {
      const body: unknown = await response.json();
      if (
        typeof body === "object" &&
        body !== null &&
        "error" in body &&
        typeof body.error === "object" &&
        body.error !== null &&
        "code" in body.error &&
        typeof body.error.code === "string"
      ) {
        code = body.error.code;
      }
    } catch {
      /* 不信任远端错误正文，界面只翻译固定错误码。 */
    }
    const kind: RequestFailure =
      response.status === 401
        ? "signedOut"
        : code === "CSRF_REJECTED"
          ? "csrfRejected"
          : response.status === 403
            ? "forbidden"
            : response.status === 400
              ? "invalid"
              : "unavailable";
    throw new DhtRequestError(kind, code);
  }
  return response.json() as Promise<T>;
}
