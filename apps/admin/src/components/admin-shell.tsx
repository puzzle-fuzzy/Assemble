import { Avatar, AvatarFallback, AvatarImage } from "@assemble/ui/avatar";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@assemble/ui/breadcrumb";
import { Button } from "@assemble/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
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
  SidebarProvider,
} from "@assemble/ui/sidebar";
import {
  ChartNoAxesCombinedIcon,
  ChevronDownIcon,
  LogOutIcon,
  LogsIcon,
  MenuIcon,
  MonitorIcon,
  UsersIcon,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { AccessFailure, AdminSession } from "../lib/api";
import { DevicesPage, LoginsPage, OverviewPage, UsersPage } from "./pages";

const navigation = [
  { key: "overview", name: "概览", icon: ChartNoAxesCombinedIcon },
  { key: "users", name: "用户", icon: UsersIcon },
  { key: "devices", name: "设备", icon: MonitorIcon },
  { key: "logins", name: "登录记录", icon: LogsIcon },
] as const;

type View = (typeof navigation)[number]["key"];

function currentView(): View {
  const hash = window.location.hash.slice(1);
  return navigation.find((item) => item.key === hash)?.key ?? "overview";
}

export function AdminShell({
  session,
  onAccessFailure,
}: {
  session: AdminSession;
  onAccessFailure: (failure: AccessFailure) => void;
}) {
  const [view, setView] = useState<View>(currentView);
  const content = useRef<HTMLDivElement>(null);
  const initial = useRef(true);
  const logout = useRef<HTMLFormElement>(null);
  const focusPageOnMenuClose = useRef(false);
  const selected =
    navigation.find((item) => item.key === view) ?? navigation[0];

  useEffect(() => {
    const update = () => setView(currentView());
    window.addEventListener("hashchange", update);
    window.addEventListener("popstate", update);
    return () => {
      window.removeEventListener("hashchange", update);
      window.removeEventListener("popstate", update);
    };
  }, []);
  useEffect(() => {
    document.title = `${selected.name} · Assemble 管理控制台`;
    if (initial.current) initial.current = false;
    else content.current?.querySelector<HTMLElement>("h1")?.focus();
  }, [selected.name]);

  function navigate(next: View) {
    if (next === view) return;
    window.history.pushState(null, "", `#${next}`);
    setView(next);
  }

  return (
    // 沿用官方 sidebar-13 的内部骨架；按用户要求移出演示对话框，右侧使用整页空间。
    <SidebarProvider className="items-start">
      <Sidebar collapsible="none" className="sticky top-0 hidden h-svh md:flex">
        <SidebarContent>
          <SidebarGroup>
            <SidebarGroupContent>
              <nav aria-label="管理功能">
                <SidebarMenu>
                  {navigation.map((item) => (
                    <SidebarMenuItem key={item.key}>
                      <SidebarMenuButton asChild isActive={view === item.key}>
                        <a
                          href={`#${item.key}`}
                          aria-current={view === item.key ? "page" : undefined}
                          onClick={(event) => {
                            event.preventDefault();
                            navigate(item.key);
                          }}
                        >
                          <item.icon aria-hidden="true" />
                          <span>{item.name}</span>
                        </a>
                      </SidebarMenuButton>
                    </SidebarMenuItem>
                  ))}
                </SidebarMenu>
              </nav>
            </SidebarGroupContent>
          </SidebarGroup>
        </SidebarContent>
      </Sidebar>
      <main className="flex h-svh min-w-0 flex-1 flex-col overflow-hidden">
        <header className="flex h-16 shrink-0 items-center justify-between gap-2 transition-[width,height] ease-linear group-has-data-[collapsible=icon]/sidebar-wrapper:h-12">
          <div className="flex min-w-0 items-center gap-2 px-4">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  className="md:hidden"
                  aria-label="打开管理菜单"
                >
                  <MenuIcon aria-hidden="true" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent
                align="start"
                onCloseAutoFocus={(event) => {
                  if (focusPageOnMenuClose.current) {
                    event.preventDefault();
                    focusPageOnMenuClose.current = false;
                    content.current?.querySelector<HTMLElement>("h1")?.focus();
                  }
                }}
              >
                <DropdownMenuRadioGroup value={view}>
                  {navigation.map((item) => (
                    <DropdownMenuRadioItem
                      key={item.key}
                      value={item.key}
                      onSelect={() => {
                        focusPageOnMenuClose.current = true;
                        navigate(item.key);
                      }}
                    >
                      <item.icon aria-hidden="true" />
                      <span>{item.name}</span>
                    </DropdownMenuRadioItem>
                  ))}
                </DropdownMenuRadioGroup>
              </DropdownMenuContent>
            </DropdownMenu>
            <Breadcrumb>
              <BreadcrumbList>
                <BreadcrumbItem className="hidden md:block">
                  <BreadcrumbLink
                    href="#overview"
                    onClick={(event) => {
                      event.preventDefault();
                      navigate("overview");
                    }}
                  >
                    Assemble 管理
                  </BreadcrumbLink>
                </BreadcrumbItem>
                <BreadcrumbSeparator className="hidden md:block" />
                <BreadcrumbItem>
                  <BreadcrumbPage>{selected.name}</BreadcrumbPage>
                </BreadcrumbItem>
              </BreadcrumbList>
            </Breadcrumb>
          </div>
          <div className="min-w-0 shrink-0 px-4">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="ghost" size="sm" aria-label="管理员账户菜单">
                  <Avatar className="size-6">
                    {session.account.avatarUrl && (
                      <AvatarImage src={session.account.avatarUrl} alt="" />
                    )}
                    <AvatarFallback>
                      {session.account.displayName.slice(0, 1)}
                    </AvatarFallback>
                  </Avatar>
                  <span className="hidden max-w-36 truncate sm:inline">
                    {session.account.displayName}
                  </span>
                  <ChevronDownIcon data-icon="inline-end" aria-hidden="true" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="max-w-72">
                <DropdownMenuLabel className="break-all">
                  {session.account.email ?? session.account.displayName}
                </DropdownMenuLabel>
                <DropdownMenuSeparator />
                <DropdownMenuGroup>
                  <DropdownMenuItem
                    onSelect={() => logout.current?.requestSubmit()}
                  >
                    <LogOutIcon aria-hidden="true" />
                    退出登录
                  </DropdownMenuItem>
                </DropdownMenuGroup>
              </DropdownMenuContent>
            </DropdownMenu>
            <form
              ref={logout}
              method="post"
              action="/web/logout?next=/admin/"
              hidden
            >
              <input type="hidden" name="csrf" value={session.csrfToken} />
            </form>
          </div>
        </header>
        <div
          ref={content}
          className="admin-content flex min-w-0 flex-1 flex-col gap-4 overflow-y-auto p-4 pt-0 [&>*]:shrink-0"
        >
          {view === "overview" && (
            <OverviewPage onAccessFailure={onAccessFailure} />
          )}
          {view === "users" && <UsersPage onAccessFailure={onAccessFailure} />}
          {view === "devices" && (
            <DevicesPage onAccessFailure={onAccessFailure} />
          )}
          {view === "logins" && (
            <LoginsPage onAccessFailure={onAccessFailure} />
          )}
        </div>
      </main>
    </SidebarProvider>
  );
}
