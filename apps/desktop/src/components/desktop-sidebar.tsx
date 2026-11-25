// 与管理端沿用 sidebar-13 的文字菜单；窄窗口通过共享菜单组件切换整页内容。
import { Button } from "@assemble/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@assemble/ui/dropdown-menu";
import {
  Sidebar,
  SidebarContent,
  SidebarGroup,
  SidebarGroupContent,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@assemble/ui/sidebar";
import {
  ArrowLeftRight,
  CalendarClock,
  Cloud,
  MenuIcon,
  Settings2,
  UserRound,
} from "lucide-react";
import { useRef } from "react";

export const DESKTOP_MODULES = [
  { id: "transfers", label: "文件传输", icon: ArrowLeftRight },
  { id: "drive", label: "云盘", icon: Cloud },
  { id: "planner", label: "日程与提醒", icon: CalendarClock },
  { id: "settings", label: "本机设置", icon: Settings2 },
  { id: "account", label: "账户中心", icon: UserRound },
] as const;
export type DesktopModule = (typeof DESKTOP_MODULES)[number]["id"];

interface NavigationProps {
  section: DesktopModule;
  onNavigate: (section: DesktopModule) => void;
}

export function DesktopSidebar({ section, onNavigate }: NavigationProps) {
  return (
    <Sidebar
      collapsible="none"
      className="sticky top-0 hidden h-svh md:flex"
      aria-label="功能侧栏"
      role="complementary"
    >
      <SidebarContent>
        <SidebarGroup>
          <SidebarGroupContent>
            <nav aria-label="主功能">
              <SidebarMenu>
                {DESKTOP_MODULES.map(({ id, label, icon: Icon }) => (
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
    </Sidebar>
  );
}

export function DesktopMobileMenu({
  section,
  onNavigate,
  onNavigateFocus,
}: NavigationProps & { onNavigateFocus: () => void }) {
  const focusPageOnClose = useRef(false);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          className="md:hidden"
          aria-label="打开功能菜单"
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
          {DESKTOP_MODULES.map(({ id, label, icon: Icon }) => (
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
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
