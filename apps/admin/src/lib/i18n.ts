import { i18n, registerTranslations, useTranslation } from "@assemble/display";
import en from "../locales/en.json";
import zhCN from "../locales/zh-CN.json";

// 先注册应用字典，再渲染依赖它的页面；语言和外观偏好统一由共享包维护。
registerTranslations("admin", { "zh-CN": zhCN, en });

export function useAdminTranslation() {
  return useTranslation("admin");
}

export function adminText(key: string, options: Record<string, unknown> = {}) {
  return i18n.t(key, { ...options, ns: "admin" });
}
