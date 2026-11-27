import { registerTranslations, useTranslation } from "@assemble/display";
import en from "../locales/activity.en.json";
import zhCN from "../locales/activity.zh-CN.json";

registerTranslations("activity", { "zh-CN": zhCN, en });
export function useActivityTranslation() {
  return useTranslation("activity");
}
