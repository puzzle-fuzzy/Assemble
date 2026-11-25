import { registerTranslations, useTranslation } from "@assemble/display";

registerTranslations("ui", {
  "zh-CN": {
    preferences: "语言与外观",
    breadcrumb: "当前位置",
    more: "更多",
    language: "语言",
    appearance: "外观",
    light: "浅色",
    dark: "深色",
    system: "跟随系统",
    close: "关闭",
    closeToast: "关闭提示",
    notifications: "通知",
    toggleSidebar: "切换侧边栏",
    sidebar: "侧边栏",
    sidebarDescription: "显示侧边栏菜单。",
  },
  en: {
    preferences: "Language and appearance",
    breadcrumb: "Breadcrumb",
    more: "More",
    language: "Language",
    appearance: "Appearance",
    light: "Light",
    dark: "Dark",
    system: "System",
    close: "Close",
    closeToast: "Dismiss notification",
    notifications: "Notifications",
    toggleSidebar: "Toggle sidebar",
    sidebar: "Sidebar",
    sidebarDescription: "Display the sidebar menu.",
  },
});

export function useUiTranslation() {
  return useTranslation("ui");
}
