import { i18n, registerTranslations, useTranslation } from "@assemble/display";
import en from "./locales/en.json";
import zhCN from "./locales/zh-CN.json";

registerTranslations("browser", { "zh-CN": zhCN, en });
export const useBrowserTranslation = () => useTranslation("browser");
export const t = (key: string) => i18n.t(key, { ns: "browser" });
export function errorText(code: string) {
  return t(`error.${Object.hasOwn(en.error, code) ? code : "unknown"}`);
}
