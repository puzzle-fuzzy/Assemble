export type Language = "zh-CN" | "en";
export type Theme = "light" | "dark" | "system";
export type ResolvedTheme = Exclude<Theme, "system">;
export interface DisplayPreferences {
  language: Language;
  theme: Theme;
  resolvedTheme: ResolvedTheme;
}

export const DISPLAY_STORAGE_KEY = "assemble.display.v1";

// 此函数也会作为独立的同步脚本运行；不能引用模块外部的变量。
export function bootstrapDisplay(
  win: Window,
  doc: Document,
  storageKey: string,
): DisplayPreferences {
  let language: Language = "zh-CN";
  let theme: Theme = "system";
  try {
    const stored: unknown = JSON.parse(
      win.localStorage.getItem(storageKey) ?? "null",
    );
    if (stored && typeof stored === "object") {
      if ("language" in stored && stored.language === "en") language = "en";
      if (
        "theme" in stored &&
        (stored.theme === "light" || stored.theme === "dark")
      )
        theme = stored.theme;
    }
  } catch {
    // 存储被禁用或旧值损坏时，仍可使用默认外观。
  }
  const dark =
    typeof win.matchMedia === "function" &&
    win.matchMedia("(prefers-color-scheme: dark)").matches;
  const resolvedTheme = theme === "system" ? (dark ? "dark" : "light") : theme;
  doc.documentElement.classList.remove("light", "dark");
  doc.documentElement.classList.add(resolvedTheme);
  const colorScheme = theme === "system" ? "light dark" : theme;
  doc.documentElement.style.colorScheme = colorScheme;
  doc
    .querySelector('meta[name="color-scheme"]')
    ?.setAttribute("content", colorScheme);
  doc.documentElement.lang = language;
  doc.documentElement.dir = "ltr";
  return { language, theme, resolvedTheme };
}
