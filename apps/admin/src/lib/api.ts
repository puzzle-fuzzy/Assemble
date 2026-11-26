// 字段与 assemble-protocol/src/admin.rs 保持一致；不读取或保存浏览器会话 Cookie。
export interface AdminAccount {
  id: string;
  displayName: string;
  email: string | null;
  avatarUrl: string | null;
}

export interface AdminSession {
  account: AdminAccount;
  csrfToken: string;
}

export interface AdminOverview {
  users: number;
  devices: number;
  onlineDevices: number;
  registrationsToday: number;
  storage: AdminStorage;
}

export interface AdminStorageAccount extends AdminAccount {
  contentBytes: number;
}

export interface AdminStorage {
  contentBytes: number;
  budgetBytes: number;
  databaseBytes: number | null;
  diskAvailableBytes: number | null;
  warningPercent: number;
  budgetWarning: boolean;
  diskWarning: boolean;
  topAccounts: AdminStorageAccount[];
}

export interface AdminUser extends AdminAccount {
  createdAt: string;
  lastLoginAt: string | null;
  hasGithub: boolean;
  isSuperAdmin: boolean;
  suspended: boolean;
  deviceCount: number;
  onlineDeviceCount: number;
}

export interface AdminDevice {
  id: string;
  accountId: string;
  accountDisplayName: string;
  accountEmail: string | null;
  name: string;
  platform: "windows" | "macos" | "linux" | "iphone" | "other";
  online: boolean;
  createdAt: string;
  lastSeenAt: string | null;
}

export interface AdminDriveFile {
  id: string;
  accountId: string;
  accountDisplayName: string;
  accountEmail: string | null;
  name: string;
  size: number;
  mimeType: string | null;
  createdAt: string;
}

export interface AdminLogin {
  id: string;
  accountId: string;
  displayName: string;
  email: string | null;
  method: "email" | "github";
  occurredAt: string;
}

export interface AdminPage<T> {
  items: T[];
  total: number;
  page: number;
  pageSize: number;
}

export interface AdminUserDetail {
  user: AdminUser;
  devices: AdminDevice[];
  logins: AdminLogin[];
}

export type AccessFailure = "signedOut" | "forbidden";
export type RequestFailure =
  | AccessFailure
  | "notFound"
  | "unavailable"
  | "invalid"
  | "csrfRejected";

export class AdminRequestError extends Error {
  constructor(readonly kind: RequestFailure) {
    super("ADMIN_REQUEST_FAILED");
  }
}

export async function request<T>(
  path: string,
  signal: AbortSignal,
  write?: { csrfToken: string; body?: unknown },
): Promise<T> {
  const response = await fetch(path, {
    method: write ? "POST" : "GET",
    credentials: "same-origin",
    cache: "no-store",
    headers: {
      Accept: "application/json",
      ...(write ? { "X-CSRF-Token": write.csrfToken } : {}),
      ...(write?.body !== undefined
        ? { "Content-Type": "application/json" }
        : {}),
    },
    redirect: "error",
    signal,
    ...(write?.body !== undefined ? { body: JSON.stringify(write.body) } : {}),
  });
  if (!response.ok) {
    let csrfRejected = false;
    if (response.status === 403) {
      try {
        const body: unknown = await response.json();
        csrfRejected =
          typeof body === "object" &&
          body !== null &&
          "error" in body &&
          typeof body.error === "object" &&
          body.error !== null &&
          "code" in body.error &&
          body.error.code === "CSRF_REJECTED";
      } catch {
        /* 错误正文不可信，不将诊断放进界面。 */
      }
    }
    const kind: RequestFailure =
      response.status === 401
        ? "signedOut"
        : csrfRejected
          ? "csrfRejected"
          : response.status === 403
            ? "forbidden"
            : response.status === 404
              ? "notFound"
              : response.status === 400
                ? "invalid"
                : "unavailable";
    throw new AdminRequestError(kind);
  }
  return response.json() as Promise<T>;
}

export function listPath(
  name: "users" | "devices" | "logins" | "drive/files",
  query: string,
  page: number,
  pageSize = 20,
) {
  const parameters = new URLSearchParams({
    q: query,
    page: String(page),
    pageSize: String(pageSize),
  });
  return `/v1/admin/${name}?${parameters}`;
}
