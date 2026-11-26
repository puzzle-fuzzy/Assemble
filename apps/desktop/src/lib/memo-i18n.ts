import { i18n, registerTranslations, useTranslation } from "@assemble/display";
import en from "@/locales/memos.en.json";
import zhCN from "@/locales/memos.zh-CN.json";

registerTranslations("memos", { "zh-CN": zhCN, en });
export const useMemoTranslation = () => useTranslation("memos");
export const memoText = (key: string) => i18n.t(key, { ns: "memos" });
