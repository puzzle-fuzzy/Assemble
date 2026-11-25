// 旧快照的 Notice 没有错误码，只识别应用壳明确生成的完整句式；不把任意诊断当作可展示文字。
import { text } from "@/lib/text-i18n";
import type { Notice } from "@/lib/types";
import zhCN from "@/locales/text.zh-CN.json";

const FIXED = new Map(
  Object.entries(zhCN.notice.fixed).map(([key, message]) => [message, key]),
);

const FILE_PATTERNS = [
  { suffix: "」超过 100 MB，发不出去。", key: "notice.fileTooLarge" },
  {
    suffix: "」读不出来，可能已被移走或没有权限。",
    key: "notice.fileUnreadable",
  },
] as const;

export function noticeText(notice: Notice): string {
  const key = FIXED.get(notice.message);
  if (key !== undefined) return text(`notice.fixed.${key}`);

  for (const pattern of FILE_PATTERNS) {
    // 必须从开头的文件名括号到结尾的系统句式完全匹配；文件名里的文字和括号不作替换。
    if (
      !notice.message.startsWith("「") ||
      !notice.message.endsWith(pattern.suffix)
    )
      continue;
    const name = notice.message.slice(1, -pattern.suffix.length);
    if (name.length > 0) return text(pattern.key, { name });
  }

  // 两种语言都保留已知提示的含义；未知原文没有可信的文案边界，不能回显潜在的 Rust 诊断。
  return text(
    notice.kind === "info" ? "notice.unknownInfo" : "notice.unknownError",
  );
}
