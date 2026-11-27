// 运行时守卫：确认 Rust 传来的状态确实是界面认得的形状。
//
// 类型只在编译期约束 TypeScript 这一侧。真正跨过桥接的是一段 JSON，
// 在入口处核对一次，界面就不会去渲染一个缺字段的半成品。

import type {
  AccountView,
  DeviceView,
  DraftView,
  DriveView,
  LaunchState,
  Location,
  Notice,
  NotificationView,
  PlannerView,
  Platform,
  ReminderView,
  Repeat,
  ScheduleView,
  SendMenuState,
  ServiceTarget,
  SessionView,
  Snapshot,
  StreamState,
  TransferDirection,
  TransferFailure,
  TransferPhase,
  TransferTarget,
  TransferView,
  UpdateState,
  UpdateView,
} from "@/lib/types";

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasString(record: Record<string, unknown>, key: string): boolean {
  return typeof record[key] === "string";
}

export function isPlatform(value: unknown): value is Platform {
  return (
    value === "windows" ||
    value === "macos" ||
    value === "linux" ||
    value === "ios" ||
    value === "other"
  );
}

/** 这个平台上的应用能不能收文件。与服务端同一条规则：手机应用第一期只有日程与提醒。 */
export function canReceiveFiles(platform: Platform): boolean {
  return platform === "windows" || platform === "macos" || platform === "linux";
}

export function isDeviceView(value: unknown): value is DeviceView {
  return (
    isRecord(value) &&
    hasString(value, "id") &&
    hasString(value, "name") &&
    isPlatform(value.platform) &&
    typeof value.online === "boolean"
  );
}

export function isStreamState(value: unknown): value is StreamState {
  return (
    value === "off" ||
    value === "connecting" ||
    value === "connected" ||
    value === "reconnecting"
  );
}

export function isAccountView(value: unknown): value is AccountView {
  return (
    isRecord(value) &&
    hasString(value, "id") &&
    hasString(value, "username") &&
    hasString(value, "displayName") &&
    (value.email === null || typeof value.email === "string") &&
    (value.avatarUrl === null ||
      (typeof value.avatarUrl === "string" &&
        /^https:\/\/avatars\.githubusercontent\.com\/u\/\d{1,20}\?s=96$/.test(
          value.avatarUrl,
        )))
  );
}

export function isLaunchState(value: unknown): value is LaunchState {
  return (
    value === "enabled" ||
    value === "disabled" ||
    value === "unsupported" ||
    value === "unknown"
  );
}

export function isUpdateView(value: unknown): value is UpdateView {
  if (!isRecord(value)) return false;
  return (
    [
      "unsupported",
      "idle",
      "checking",
      "upToDate",
      "downloading",
      "ready",
      "failed",
    ].includes(value.state as UpdateState) &&
    (value.version === null || typeof value.version === "string") &&
    (value.received === null || typeof value.received === "number") &&
    (value.total === null || typeof value.total === "number") &&
    (value.checkedAt === null || typeof value.checkedAt === "number") &&
    (value.error === null || typeof value.error === "string")
  );
}

export function isServiceTarget(value: unknown): value is ServiceTarget {
  return value === "production" || value === "development";
}

export function isNotice(value: unknown): value is Notice {
  return (
    isRecord(value) &&
    (value.kind === "info" || value.kind === "error") &&
    hasString(value, "message")
  );
}

export function isTransferDirection(
  value: unknown,
): value is TransferDirection {
  return value === "in" || value === "out";
}

export function isTransferPhase(value: unknown): value is TransferPhase {
  return (
    value === "queued" ||
    value === "inProgress" ||
    value === "waiting" ||
    value === "complete" ||
    value === "failed" ||
    value === "declined" ||
    value === "cancelled" ||
    value === "expired"
  );
}

export function isTransferFailure(value: unknown): value is TransferFailure {
  return (
    value === "diskFull" ||
    value === "writeFailed" ||
    value === "verificationFailed" ||
    value === "cancelled" ||
    value === "deviceOffline" ||
    value === "tooLarge" ||
    value === "fileUnreadable" ||
    value === "rejected" ||
    value === "quotaExceeded" ||
    value === "vanished" ||
    value === "interrupted"
  );
}

export function isTransferTarget(value: unknown): value is TransferTarget {
  return value === "device" || value === "drive";
}

export function isTransferView(value: unknown): value is TransferView {
  return (
    isRecord(value) &&
    hasString(value, "id") &&
    isTransferDirection(value.direction) &&
    hasString(value, "peer") &&
    hasString(value, "name") &&
    typeof value.size === "number" &&
    isTransferPhase(value.phase) &&
    typeof value.progress === "number" &&
    (value.savedAs === null || typeof value.savedAs === "string") &&
    (value.failure === null || isTransferFailure(value.failure)) &&
    typeof value.retryable === "boolean" &&
    isTransferTarget(value.target)
  );
}

export function isDriveView(value: unknown): value is DriveView {
  return (
    isRecord(value) &&
    typeof value.usedBytes === "number" &&
    typeof value.limitBytes === "number" &&
    Array.isArray(value.items) &&
    value.items.every(
      (item) =>
        isRecord(item) &&
        hasString(item, "id") &&
        hasString(item, "name") &&
        hasString(item, "createdAt") &&
        typeof item.size === "number",
    )
  );
}

function isRepeat(value: unknown): value is Repeat {
  return (
    value === "none" ||
    value === "daily" ||
    value === "weekly" ||
    value === "monthly"
  );
}

function isLocation(value: unknown): value is Location {
  return (
    isRecord(value) && hasString(value, "label") && hasString(value, "address")
  );
}

function isScheduleView(value: unknown): value is ScheduleView {
  return (
    isRecord(value) &&
    hasString(value, "id") &&
    typeof value.version === "number" &&
    hasString(value, "createdBy") &&
    hasString(value, "title") &&
    (value.location === null || isLocation(value.location)) &&
    hasString(value, "startAt") &&
    (value.endAt === null || typeof value.endAt === "string") &&
    isRepeat(value.repeat) &&
    (value.repeatUntil === null || typeof value.repeatUntil === "string") &&
    (value.remindBefore === null || typeof value.remindBefore === "number") &&
    (value.notes === null || typeof value.notes === "string") &&
    Array.isArray(value.occurrences) &&
    value.occurrences.every((at) => typeof at === "string")
  );
}

function isReminderView(value: unknown): value is ReminderView {
  return (
    isRecord(value) &&
    hasString(value, "id") &&
    typeof value.version === "number" &&
    (value.status === "scheduled" || value.status === "fired") &&
    hasString(value, "createdBy") &&
    hasString(value, "title") &&
    hasString(value, "dueAt") &&
    (value.notes === null || typeof value.notes === "string")
  );
}

function isNotificationView(value: unknown): value is NotificationView {
  return (
    isRecord(value) &&
    hasString(value, "id") &&
    hasString(value, "title") &&
    (value.body === null || typeof value.body === "string") &&
    (value.scheduleId === null || typeof value.scheduleId === "string") &&
    (value.reminderId === null || typeof value.reminderId === "string") &&
    hasString(value, "raisedAt")
  );
}

function isPlannerView(value: unknown): value is PlannerView {
  return (
    isRecord(value) &&
    (value.navigation === undefined ||
      (Array.isArray(value.navigation) &&
        value.navigation.every(
          (binding) =>
            isRecord(binding) &&
            ["schedule", "reminder"].includes(String(binding.resource)) &&
            hasString(binding, "id") &&
            Array.isArray(binding.locations) &&
            binding.locations.every(isLocation),
        ))) &&
    Array.isArray(value.notifications) &&
    value.notifications.every(isNotificationView) &&
    Array.isArray(value.schedules) &&
    value.schedules.every(isScheduleView) &&
    Array.isArray(value.reminders) &&
    value.reminders.every(isReminderView) &&
    (value.focus === null || typeof value.focus === "string") &&
    (value.compose === null ||
      value.compose === "reminder" ||
      value.compose === "schedule")
  );
}

export function isSendMenuState(value: unknown): value is SendMenuState {
  return value === "enabled" || value === "disabled" || value === "unsupported";
}

export function isDraftView(value: unknown): value is DraftView {
  return (
    isRecord(value) &&
    hasString(value, "id") &&
    Array.isArray(value.files) &&
    value.files.every(
      (file) =>
        isRecord(file) &&
        hasString(file, "name") &&
        typeof file.size === "number",
    )
  );
}

export function isSessionView(value: unknown): value is SessionView {
  if (!isRecord(value)) return false;
  switch (value.phase) {
    case "signedOut":
    case "signingIn":
      return true;
    case "signedIn":
      return isAccountView(value.account) && isDeviceView(value.device);
    default:
      return false;
  }
}

import { isSystemNotifications } from "@assemble/ui/lib/system-notifications";
import { isDownloadsView } from "./downloads";
import { isRemoteView } from "./remote";

export function isSnapshot(value: unknown): value is Snapshot {
  if (!isRecord(value)) return false;
  const service = value.service;
  return (
    isRecord(service) &&
    isSystemNotifications(value.systemNotifications) &&
    isDownloadsView(value.downloads) &&
    isRemoteView(value.remote) &&
    isServiceTarget(service.target) &&
    (service.reachable === null || typeof service.reachable === "boolean") &&
    isStreamState(service.stream) &&
    isSessionView(value.session) &&
    Array.isArray(value.devices) &&
    value.devices.every(isDeviceView) &&
    Array.isArray(value.transfers) &&
    value.transfers.every(isTransferView) &&
    (value.draft === null || isDraftView(value.draft)) &&
    (value.drive === null || isDriveView(value.drive)) &&
    isSendMenuState(value.sendMenu) &&
    isRecord(value.receive) &&
    (value.receive.customFolder === null ||
      typeof value.receive.customFolder === "string") &&
    (value.receive.displayPath === null ||
      typeof value.receive.displayPath === "string") &&
    isLaunchState(value.launchAtLogin) &&
    (value.notice === null || isNotice(value.notice)) &&
    (value.planner === null || isPlannerView(value.planner)) &&
    typeof value.plannerMuted === "boolean" &&
    isUpdateView(value.update)
  );
}
