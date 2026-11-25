import { createInstance, type ResourceLanguage } from "i18next";
import { initReactI18next } from "react-i18next";
import {
  getDisplayPreferences,
  subscribeDisplayPreferences,
} from "./preferences";

export { useTranslation } from "react-i18next";
export type {
  DisplayPreferences,
  Language,
  ResolvedTheme,
  Theme,
} from "./bootstrap";
export { DISPLAY_STORAGE_KEY } from "./bootstrap";
export {
  getDisplayPreferences,
  setLanguage,
  setTheme,
  useDisplayPreferences,
} from "./preferences";

export const i18n = createInstance();
// 资源随应用打包，不请求网络，也不把用户数据发送给翻译服务。
void i18n.use(initReactI18next).init({
  lng: getDisplayPreferences().language,
  supportedLngs: ["zh-CN", "en"],
  fallbackLng: "zh-CN",
  load: "currentOnly",
  ns: [],
  resources: {},
  initAsync: false,
  interpolation: { escapeValue: false },
  react: { useSuspense: false },
});
subscribeDisplayPreferences(() => {
  const language = getDisplayPreferences().language;
  if (i18n.language !== language) void i18n.changeLanguage(language);
});

export function registerTranslations(
  namespace: string,
  resources: Record<"zh-CN" | "en", ResourceLanguage>,
) {
  for (const language of ["zh-CN", "en"] as const) {
    i18n.addResourceBundle(
      language,
      namespace,
      resources[language],
      true,
      true,
    );
  }
}
