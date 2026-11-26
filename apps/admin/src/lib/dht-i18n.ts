import { registerTranslations, useTranslation } from "@assemble/display";
import en from "../locales/dht.en.json";
import zhCN from "../locales/dht.zh-CN.json";

registerTranslations("adminDht", { "zh-CN": zhCN, en });
export function useDhtTranslation() {
  return useTranslation("adminDht");
}
