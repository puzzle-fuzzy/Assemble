// 错误码到用户可读文案的映射。
//
// Rust 传来的 `message` 是给开发看的英文诊断，不回显给用户；
// 这里只认稳定的错误码，认不出的一律用通用提示。

import type { BridgeError } from "@/lib/bridge";

const MESSAGES: Record<string, string> = {
  CREDENTIAL_STORE_UNAVAILABLE: "系统钥匙串不可用，无法保存登录状态。",
  BROWSER_UNAVAILABLE: "无法打开系统浏览器，请重试。",
  UNREACHABLE: "暂时连不上服务，请检查网络后重试。",
  ACCOUNT_DISABLED: "这个账户已被禁用。",
  UNSUPPORTED: "当前运行方式不支持设置开机启动。",
  SYSTEM_REJECTED: "系统没有接受这个设置，请稍后重试。",
};

const GENERIC = "操作没有完成，请稍后重试。";

export function messageFor(error: BridgeError): string {
  return MESSAGES[error.code] ?? GENERIC;
}
