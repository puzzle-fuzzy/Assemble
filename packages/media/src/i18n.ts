import { i18n, registerTranslations, useTranslation } from "@assemble/display";
import en from "./locales/en.json";
import zhCN from "./locales/zh-CN.json";

registerTranslations("media", { "zh-CN": zhCN, en });
export const useMediaTranslation = () => useTranslation("media");
export const mediaText = (key: string, options: Record<string, unknown> = {}) =>
  i18n.t(key, { ...options, ns: "media" });
