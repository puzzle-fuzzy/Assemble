// 辅助函数与界面使用同一个语言实例，按调用时的选择生成文字，避免模块初始化时钉死语言。
import {
  getDisplayPreferences,
  i18n,
  registerTranslations,
} from "@assemble/display";
import en from "@/locales/text.en.json";
import zhCN from "@/locales/text.zh-CN.json";

registerTranslations("desktopText", { "zh-CN": zhCN, en });

export function text(
  key: string,
  options: Record<string, unknown> = {},
): string {
  return i18n.t(key, { ...options, ns: "desktopText" });
}

/** 只决定文字与日期格式的语言，不指定或改变本机时区。 */
export function textLocale(): "zh-CN" | "en" {
  return getDisplayPreferences().language;
}
