// 只组合现有 shadcn 菜单；两端入口共享选项、选中状态与键盘操作。
import { useDisplayPreferences } from "@assemble/display";
import { LanguagesIcon, MonitorIcon, MoonIcon, SunIcon } from "lucide-react";
import { useUiTranslation } from "../lib/i18n";
import { Button } from "./button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "./dropdown-menu";

export function DisplayPreferencesItems() {
  const { t } = useUiTranslation();
  const { language, theme, setLanguage, setTheme } = useDisplayPreferences();
  const ThemeIcon =
    theme === "system" ? MonitorIcon : theme === "dark" ? MoonIcon : SunIcon;
  return (
    <DropdownMenuGroup>
      <DropdownMenuSub>
        <DropdownMenuSubTrigger>
          <LanguagesIcon aria-hidden="true" />
          {t("language")}
        </DropdownMenuSubTrigger>
        <DropdownMenuSubContent>
          <DropdownMenuRadioGroup
            aria-label={t("language")}
            value={language}
            onValueChange={(value) => {
              if (value === "zh-CN" || value === "en") setLanguage(value);
            }}
          >
            <DropdownMenuRadioItem value="zh-CN">
              <span lang="zh-CN">简体中文</span>
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="en">
              <span lang="en">English</span>
            </DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
        </DropdownMenuSubContent>
      </DropdownMenuSub>
      <DropdownMenuSub>
        <DropdownMenuSubTrigger>
          <ThemeIcon aria-hidden="true" />
          {t("appearance")}
        </DropdownMenuSubTrigger>
        <DropdownMenuSubContent>
          <DropdownMenuRadioGroup
            aria-label={t("appearance")}
            value={theme}
            onValueChange={(value) => {
              if (value === "light" || value === "dark" || value === "system")
                setTheme(value);
            }}
          >
            <DropdownMenuRadioItem value="light">
              <SunIcon aria-hidden="true" />
              {t("light")}
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="dark">
              <MoonIcon aria-hidden="true" />
              {t("dark")}
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="system">
              <MonitorIcon aria-hidden="true" />
              {t("system")}
            </DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
        </DropdownMenuSubContent>
      </DropdownMenuSub>
    </DropdownMenuGroup>
  );
}

export function DisplayPreferencesMenu() {
  const { t } = useUiTranslation();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon" aria-label={t("preferences")}>
          <LanguagesIcon />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-48">
        <DisplayPreferencesItems />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
