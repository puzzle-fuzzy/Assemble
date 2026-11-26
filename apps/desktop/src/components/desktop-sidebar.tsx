// 与管理端沿用 sidebar-13 的文字菜单；窄窗口通过共享菜单组件切换整页内容。

import { Button } from "@assemble/ui/button";
import { DisplayPreferencesItems } from "@assemble/ui/display-preferences";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@assemble/ui/dropdown-menu";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@assemble/ui/sidebar";
import { SidebarAccountMenu } from "@assemble/ui/sidebar-account-menu";
import {
  ArrowLeftRight,
  CalendarClock,
  Cloud,
  Download,
  Film,
  InfoIcon,
  LogOutIcon,
  MenuIcon,
  NotebookPen,
  Settings2,
  UserRound,
} from "lucide-react";
import { useRef } from "react";
import { downloadText } from "@/lib/download-i18n";
import { uiText, useDesktopTranslation } from "@/lib/i18n";
import { memoText } from "@/lib/memo-i18n";
import type { SessionView } from "@/lib/types";

export function desktopModules() {
  return [
    { id: "transfers", label: uiText("fileTransfers"), icon: ArrowLeftRight },
    { id: "drive", label: uiText("cloudDrive"), icon: Cloud },
    { id: "downloads", label: downloadText("title"), icon: Download },
    { id: "media", label: uiText("moviesAndTV"), icon: Film },
    { id: "memos", label: memoText("title"), icon: NotebookPen },
    {
      id: "planner",
      label: uiText("schedulesAndReminders"),
      icon: CalendarClock,
    },
    { id: "settings", label: uiText("deviceSettings"), icon: Settings2 },
    { id: "account", label: uiText("accountCenter"), icon: UserRound },
    { id: "about", label: uiText("about"), icon: InfoIcon },
  ] as const;
}
export type DesktopModule = ReturnType<typeof desktopModules>[number]["id"];

interface NavigationProps {
  section: DesktopModule;
  onNavigate: (section: DesktopModule) => void;
}

export function DesktopSidebar({
  section,
  onNavigate,
  session,
  pending,
  onSignOut,
  onNavigateFocus,
}: NavigationProps & {
  session: SessionView | undefined;
  pending: boolean;
  onSignOut: () => void;
  onNavigateFocus: () => void;
}) {
  useDesktopTranslation();
  const account = session?.phase === "signedIn" ? session.account : null;
  const identity = {
    name:
      account?.displayName ??
      (session?.phase === "signingIn"
        ? uiText("signingIn")
        : uiText("signedOut")),
    secondary:
      account?.email ??
      account?.username ??
      uiText("signInWithYourSystemBrowser"),
    avatarUrl: account?.avatarUrl ?? null,
  };
  return (
    <Sidebar
      collapsible="none"
      className="sticky top-0 hidden h-svh md:flex"
      aria-label={uiText("sidebar")}
      role="complementary"
    >
      <SidebarContent>
        <SidebarGroup>
          <SidebarGroupContent>
            <nav aria-label={uiText("mainNavigation")}>
              <SidebarMenu>
                {desktopModules()
                  .filter(
                    (item) => item.id !== "account" && item.id !== "about",
                  )
                  .map(({ id, label, icon: Icon }) => (
                    <SidebarMenuItem key={id}>
                      <SidebarMenuButton
                        onClick={() => onNavigate(id)}
                        isActive={section === id}
                        aria-label={label}
                        aria-current={section === id ? "page" : undefined}
                        aria-controls={`view-${id}`}
                      >
                        <Icon aria-hidden="true" />
                        <span>{label}</span>
                      </SidebarMenuButton>
                    </SidebarMenuItem>
                  ))}
              </SidebarMenu>
            </nav>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>
      <SidebarFooter>
        <SidebarAccountMenu
          identity={identity}
          label={uiText("accountMenu")}
          pending={pending}
          active={section === "account" || section === "about"}
          onNavigateFocus={onNavigateFocus}
          groups={[
            [
              {
                label: uiText("accountCenter"),
                icon: UserRound,
                onSelect: () => onNavigate("account"),
                focusPage: true,
                current: section === "account",
              },
              {
                label: uiText("about"),
                icon: InfoIcon,
                onSelect: () => onNavigate("about"),
                focusPage: true,
                current: section === "about",
              },
            ],
            ...(account
              ? [
                  [
                    {
                      label: uiText("signOut"),
                      icon: LogOutIcon,
                      onSelect: onSignOut,
                    },
                  ],
                ]
              : []),
          ]}
        />
      </SidebarFooter>
    </Sidebar>
  );
}

export function DesktopMobileMenu({
  section,
  onNavigate,
  onNavigateFocus,
}: NavigationProps & { onNavigateFocus: () => void }) {
  useDesktopTranslation();
  const focusPageOnClose = useRef(false);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          className="md:hidden"
          aria-label={uiText("openNavigationMenu")}
        >
          <MenuIcon aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        onCloseAutoFocus={(event) => {
          if (focusPageOnClose.current) {
            event.preventDefault();
            focusPageOnClose.current = false;
            onNavigateFocus();
          }
        }}
      >
        <DropdownMenuRadioGroup value={section}>
          {desktopModules().map(({ id, label, icon: Icon }) => (
            <DropdownMenuRadioItem
              key={id}
              value={id}
              onSelect={() => {
                focusPageOnClose.current = true;
                onNavigate(id);
              }}
            >
              <Icon aria-hidden="true" />
              <span>{label}</span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DisplayPreferencesItems />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
