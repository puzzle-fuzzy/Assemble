// 运行时守卫：确认 Rust 传来的状态确实是界面认得的形状。
//
// 类型只在编译期约束 TypeScript 这一侧。真正跨过桥接的是一段 JSON，
// 在入口处核对一次，界面就不会去渲染一个缺字段的半成品。

import type {
  AccountView,
  LaunchState,
  Notice,
  ServiceTarget,
  SessionView,
  Snapshot,
} from "@/lib/types";

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasString(record: Record<string, unknown>, key: string): boolean {
  return typeof record[key] === "string";
}

export function isAccountView(value: unknown): value is AccountView {
  return (
    isRecord(value) &&
    hasString(value, "id") &&
    hasString(value, "username") &&
    hasString(value, "displayName")
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

export function isSessionView(value: unknown): value is SessionView {
  if (!isRecord(value)) return false;
  switch (value.phase) {
    case "signedOut":
    case "signingIn":
      return true;
    case "signedIn":
      return isAccountView(value.account);
    default:
      return false;
  }
}

export function isSnapshot(value: unknown): value is Snapshot {
  if (!isRecord(value)) return false;
  const service = value.service;
  return (
    isRecord(service) &&
    isServiceTarget(service.target) &&
    (service.reachable === null || typeof service.reachable === "boolean") &&
    isSessionView(value.session) &&
    isLaunchState(value.launchAtLogin) &&
    (value.notice === null || isNotice(value.notice))
  );
}
