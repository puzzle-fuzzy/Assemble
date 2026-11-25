// 桌面系统文案只使用本地资源；文件名、账户和日程内容保持原样。
import { i18n, registerTranslations, useTranslation } from "@assemble/display";
import en from "@/locales/en.json";
import zhCN from "@/locales/zh-CN.json";

registerTranslations("desktop", { "zh-CN": zhCN, en });

export function useDesktopTranslation() {
  return useTranslation("desktop");
}

export function uiText(
  key: string,
  options: Record<string, unknown> = {},
): string {
  return i18n.t(key, { ...options, ns: "desktop" });
}

export function formatStoredDate(iso: string): string {
  return new Intl.DateTimeFormat(
    i18n.resolvedLanguage === "en" ? "en" : "zh-CN",
    {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      timeZone: "UTC",
    },
  ).format(new Date(iso));
}
