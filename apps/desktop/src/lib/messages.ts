// 错误码到用户可读文案的映射。
//
// Rust 传来的 `message` 是给开发看的英文诊断，不回显给用户；
// 这里只认稳定的错误码，认不出的一律用通用提示。

import type { BridgeError } from "@/lib/bridge";
import { text } from "@/lib/text-i18n";
import zhCN from "@/locales/text.zh-CN.json";

export function messageFor(error: BridgeError): string {
  const code = Object.hasOwn(zhCN.errors, error.code) ? error.code : "GENERIC";
  return text(`errors.${code}`);
}
