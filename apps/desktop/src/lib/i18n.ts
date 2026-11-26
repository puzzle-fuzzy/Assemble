// 桌面系统文案只使用本地资源；文件名、账户和日程内容保持原样。
import { i18n, registerTranslations, useTranslation } from "@assemble/display";
import type { Platform } from "@/lib/types";
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

/** 平台的显示名。几种机器的名字各语言都一样；认不出的那一种按界面语言说。 */
export function platformName(platform: Platform): string {
  switch (platform) {
    case "windows":
      return "Windows";
    case "macos":
      return "Mac";
    case "linux":
      return "Linux";
    case "ios":
      return "iPhone";
    case "other":
      return uiText("platformOther");
  }
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
