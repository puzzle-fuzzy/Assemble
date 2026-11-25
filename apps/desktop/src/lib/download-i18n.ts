import { i18n, registerTranslations, useTranslation } from "@assemble/display";
import en from "@/locales/downloads.en.json";
import zhCN from "@/locales/downloads.zh-CN.json";

registerTranslations("downloads", { "zh-CN": zhCN, en });
export const useDownloadTranslation = () => useTranslation("downloads");
export const downloadText = (
  key: string,
  options: Record<string, unknown> = {},
) => i18n.t(key, { ...options, ns: "downloads" });
export function downloadError(code: string): string {
  return downloadText(
    `error.${Object.hasOwn(en.error, code) ? code : "unknown"}`,
  );
}
