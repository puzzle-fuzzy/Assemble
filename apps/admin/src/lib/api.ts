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
}

export interface AdminUser extends AdminAccount {
  createdAt: string;
  lastLoginAt: string | null;
  hasGithub: boolean;
  isSuperAdmin: boolean;
  deviceCount: number;
  onlineDeviceCount: number;
}

export interface AdminDevice {
  id: string;
  accountId: string;
  accountDisplayName: string;
  accountEmail: string | null;
  name: string;
  platform: "windows" | "macos" | "linux" | "other";
  online: boolean;
  createdAt: string;
  lastSeenAt: string | null;
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
export type RequestFailure = AccessFailure | "notFound" | "unavailable";

export class AdminRequestError extends Error {
  constructor(readonly kind: RequestFailure) {
    super("ADMIN_REQUEST_FAILED");
  }
}

export async function request<T>(
  path: string,
  signal: AbortSignal,
): Promise<T> {
  const response = await fetch(path, {
    method: "GET",
    credentials: "same-origin",
    cache: "no-store",
    headers: { Accept: "application/json" },
    redirect: "error",
    signal,
  });
  if (!response.ok) {
    const kind: RequestFailure =
      response.status === 401
        ? "signedOut"
        : response.status === 403
          ? "forbidden"
          : response.status === 404
            ? "notFound"
            : "unavailable";
    throw new AdminRequestError(kind);
  }
  return response.json() as Promise<T>;
}

export function listPath(
  name: "users" | "devices" | "logins",
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
