import { useSyncExternalStore } from "react";
import {
  bootstrapDisplay,
  DISPLAY_STORAGE_KEY,
  type DisplayPreferences,
  type Language,
  type Theme,
} from "./bootstrap";

export function createDisplayStore(win?: Window, doc?: Document) {
  const media =
    win && typeof win.matchMedia === "function"
      ? win.matchMedia("(prefers-color-scheme: dark)")
      : undefined;
  let current: DisplayPreferences =
    win && doc
      ? bootstrapDisplay(win, doc, DISPLAY_STORAGE_KEY)
      : { language: "zh-CN", theme: "system", resolvedTheme: "light" };
  const listeners = new Set<() => void>();

  function getDisplayPreferences(): DisplayPreferences {
    return current;
  }

  function subscribeDisplayPreferences(listener: () => void) {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }

  function apply(language: Language, theme: Theme) {
    const resolvedTheme =
      theme === "system" ? (media?.matches ? "dark" : "light") : theme;
    if (doc) {
      const root = doc.documentElement;
      root.classList.remove("light", "dark");
      root.classList.add(resolvedTheme);
      const colorScheme = theme === "system" ? "light dark" : theme;
      root.style.colorScheme = colorScheme;
      doc
        .querySelector('meta[name="color-scheme"]')
        ?.setAttribute("content", colorScheme);
      root.lang = language;
      root.dir = "ltr";
    }
    if (
      current.language === language &&
      current.theme === theme &&
      current.resolvedTheme === resolvedTheme
    )
      return;
    current = { language, theme, resolvedTheme };
    for (const listener of listeners) listener();
  }

  function persist() {
    if (!win) return;
    try {
      // 跟随系统不保存主题覆盖值，以便系统变化继续生效。
      const value = {
        ...(current.language !== "zh-CN" ? { language: current.language } : {}),
        ...(current.theme !== "system" ? { theme: current.theme } : {}),
      };
      if (Object.keys(value).length)
        win.localStorage.setItem(DISPLAY_STORAGE_KEY, JSON.stringify(value));
      else win.localStorage.removeItem(DISPLAY_STORAGE_KEY);
    } catch {
      // 本机禁用持久化时，本窗口的选择仍然有效。
    }
  }

  function setLanguage(language: Language) {
    if (language !== "zh-CN" && language !== "en") return;
    apply(language, current.theme);
    persist();
  }

  function setTheme(theme: Theme) {
    if (theme !== "light" && theme !== "dark" && theme !== "system") return;
    apply(current.language, theme);
    persist();
  }

  const onMediaChange = () => {
    if (current.theme === "system") apply(current.language, current.theme);
  };
  const onStorage = (event: StorageEvent) => {
    if (event.key !== DISPLAY_STORAGE_KEY && event.key !== null) return;
    if (!win || !doc) return;
    try {
      if (event.storageArea && event.storageArea !== win.localStorage) return;
    } catch {
      return;
    }
    // 使用实际存储值，忽略排队中的旧事件，且不把收到的变化写回。
    const preferences = bootstrapDisplay(win, doc, DISPLAY_STORAGE_KEY);
    apply(preferences.language, preferences.theme);
  };
  media?.addEventListener("change", onMediaChange);
  win?.addEventListener("storage", onStorage);
  return {
    getDisplayPreferences,
    subscribeDisplayPreferences,
    setLanguage,
    setTheme,
    dispose() {
      media?.removeEventListener("change", onMediaChange);
      win?.removeEventListener("storage", onStorage);
      listeners.clear();
    },
  };
}

const browser =
  typeof window !== "undefined" && typeof document !== "undefined";
const store = createDisplayStore(
  browser ? window : undefined,
  browser ? document : undefined,
);
export const {
  getDisplayPreferences,
  subscribeDisplayPreferences,
  setLanguage,
  setTheme,
} = store;

export function useDisplayPreferences() {
  const preferences = useSyncExternalStore(
    subscribeDisplayPreferences,
    getDisplayPreferences,
    getDisplayPreferences,
  );
  return { ...preferences, setLanguage, setTheme };
}
