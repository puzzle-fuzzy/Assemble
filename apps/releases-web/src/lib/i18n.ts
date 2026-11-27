import { registerTranslations, useTranslation } from "@assemble/display";
import en from "../locales/en.json";
import zhCN from "../locales/zh-CN.json";

registerTranslations("releases", { "zh-CN": zhCN, en });
export const useReleasesTranslation = () => useTranslation("releases");
