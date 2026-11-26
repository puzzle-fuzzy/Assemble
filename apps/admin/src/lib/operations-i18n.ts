import { registerTranslations, useTranslation } from "@assemble/display";
import en from "../locales/operations.en.json";
import zhCN from "../locales/operations.zh-CN.json";

registerTranslations("adminOperations", { "zh-CN": zhCN, en });

export function useOperationsTranslation() {
  return useTranslation("adminOperations");
}
