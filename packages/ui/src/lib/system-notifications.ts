// 与 assemble-protocol/system_notifications.rs 对齐；不含路径、来源地址或诊断。
export type SystemNotificationContent =
  | {
      type: "storageBudget";
      usedBytes: number;
      budgetBytes: number;
      warningPercent: number;
    }
  | { type: "storageDisk"; availableBytes: number }
  | { type: "storageUnavailable" }
  | {
      type: "transferFailed";
      transferId: string;
      name: string;
      localDiskFull: boolean;
    }
  | {
      type: "downloadFailed";
      taskId: string;
      name: string;
      localDiskFull: boolean;
    }
  | { type: "sessionExpired" };

export interface SystemNotification {
  id: string;
  revision: number;
  content: SystemNotificationContent;
  level: "warning" | "error";
  createdAt: string;
  updatedAt: string;
  resolvedAt: string | null;
  readAt: string | null;
}
export interface SystemNotifications {
  items: SystemNotification[];
  available: boolean;
}
export interface SystemNotificationRead {
  id: string;
  revision: number;
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function timestamp(value: unknown) {
  return (
    typeof value === "string" &&
    /^\d{4}-\d\d-\d\dT/.test(value) &&
    Number.isFinite(Date.parse(value))
  );
}
export function isSystemNotifications(
  value: unknown,
): value is SystemNotifications {
  return (
    record(value) &&
    typeof value.available === "boolean" &&
    Array.isArray(value.items) &&
    value.items.length <= 100 &&
    value.items.every(isSystemNotification)
  );
}
export function isSystemNotification(
  value: unknown,
): value is SystemNotification {
  if (
    !record(value) ||
    typeof value.id !== "string" ||
    !Number.isSafeInteger(value.revision) ||
    (value.revision as number) < 1 ||
    !["warning", "error"].includes(String(value.level)) ||
    !timestamp(value.createdAt) ||
    !timestamp(value.updatedAt) ||
    (value.resolvedAt !== null && !timestamp(value.resolvedAt)) ||
    (value.readAt !== null && !timestamp(value.readAt)) ||
    !record(value.content)
  )
    return false;
  const content = value.content;
  switch (content.type) {
    case "storageBudget":
      return [
        content.usedBytes,
        content.budgetBytes,
        content.warningPercent,
      ].every((n) => typeof n === "number" && Number.isFinite(n) && n >= 0);
    case "storageDisk":
      return (
        typeof content.availableBytes === "number" &&
        Number.isFinite(content.availableBytes) &&
        content.availableBytes >= 0
      );
    case "storageUnavailable":
    case "sessionExpired":
      return true;
    case "transferFailed":
      return (
        typeof content.transferId === "string" &&
        typeof content.name === "string" &&
        typeof content.localDiskFull === "boolean"
      );
    case "downloadFailed":
      return (
        typeof content.taskId === "string" &&
        typeof content.name === "string" &&
        typeof content.localDiskFull === "boolean"
      );
    default:
      return false;
  }
}
