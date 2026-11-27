import { registerTranslations, useTranslation } from "@assemble/display";
import en from "@/locales/drive.en.json";
import zhCN from "@/locales/drive.zh-CN.json";

registerTranslations("desktopDrive", { "zh-CN": zhCN, en });
export const useDriveTranslation = () => useTranslation("desktopDrive");
