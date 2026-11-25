import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@assemble/ui/breadcrumb";
import { Button } from "@assemble/ui/button";
import { DisplayPreferencesItems } from "@assemble/ui/display-preferences";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
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
  SidebarProvider,
} from "@assemble/ui/sidebar";
import { SidebarAccountMenu } from "@assemble/ui/sidebar-account-menu";
import {
  ChartNoAxesCombinedIcon,
  LogOutIcon,
  LogsIcon,
  MenuIcon,
  MonitorIcon,
  UsersIcon,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { AccessFailure, AdminSession } from "../lib/api";
import { useAdminTranslation } from "../lib/i18n";
import { DevicesPage, LoginsPage, OverviewPage, UsersPage } from "./pages";

const navigation = [
  { key: "overview", icon: ChartNoAxesCombinedIcon },
  { key: "users", icon: UsersIcon },
  { key: "devices", icon: MonitorIcon },
  { key: "logins", icon: LogsIcon },
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
  const { t } = useAdminTranslation();
  const [view, setView] = useState<View>(currentView);
  const pageHeading = useRef<HTMLSpanElement>(null);
  const [userDetail, setUserDetail] = useState(false);
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
  const pageName = t(
    view === "users" && userDetail
      ? "navigation.userDetail"
      : `navigation.${selected.key}`,
  );
  useEffect(() => {
    document.title = t("app.documentTitle", { page: pageName });
  }, [pageName, t]);
  const focusKey = view === "users" && userDetail ? "user-detail" : view;
  const previousFocusKey = useRef(focusKey);
  useEffect(() => {
    // 切换语言只更新文案，不把焦点从偏好菜单或搜索框抢到页面标题。
    if (previousFocusKey.current !== focusKey) {
      previousFocusKey.current = focusKey;
      pageHeading.current?.focus();
    }
  }, [focusKey]);

  function navigate(next: View) {
    if (next === view) return;
    window.history.pushState(null, "", `#${next}`);
    setUserDetail(false);
    setView(next);
  }

  return (
    // 沿用官方 sidebar-13 的内部骨架；按用户要求移出演示对话框，右侧使用整页空间。
    <SidebarProvider className="items-start">
      <form ref={logout} method="post" action="/web/logout?next=/admin/" hidden>
        <input type="hidden" name="csrf" value={session.csrfToken} />
      </form>
      <Sidebar collapsible="none" className="sticky top-0 hidden h-svh md:flex">
        <SidebarContent>
          <SidebarGroup>
            <SidebarGroupContent>
              <nav aria-label={t("navigation.label")}>
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
                          <span>{t(`navigation.${item.key}`)}</span>
                        </a>
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
            identity={{
              name: session.account.displayName,
              secondary: session.account.email ?? session.account.displayName,
              avatarUrl: session.account.avatarUrl,
            }}
            label={t("navigation.accountMenu")}
            groups={[
              [
                {
                  label: t("navigation.signOut"),
                  icon: LogOutIcon,
                  onSelect: () => logout.current?.requestSubmit(),
                },
              ],
            ]}
          />
        </SidebarFooter>
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
                  aria-label={t("navigation.openMenu")}
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
                    pageHeading.current?.focus();
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
                      <span>{t(`navigation.${item.key}`)}</span>
                    </DropdownMenuRadioItem>
                  ))}
                </DropdownMenuRadioGroup>
                <DropdownMenuSeparator />
                <DisplayPreferencesItems />
                <DropdownMenuSeparator />
                <DropdownMenuGroup>
                  <DropdownMenuItem
                    onSelect={() => logout.current?.requestSubmit()}
                  >
                    <LogOutIcon aria-hidden="true" />
                    {t("navigation.signOut")}
                  </DropdownMenuItem>
                </DropdownMenuGroup>
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
                    {t("app.brand")}
                  </BreadcrumbLink>
                </BreadcrumbItem>
                <BreadcrumbSeparator className="hidden md:block" />
                <BreadcrumbItem>
                  <BreadcrumbPage
                    ref={pageHeading}
                    role="heading"
                    aria-level={1}
                    tabIndex={-1}
                  >
                    {pageName}
                  </BreadcrumbPage>
                </BreadcrumbItem>
              </BreadcrumbList>
            </Breadcrumb>
          </div>
        </header>
        {/* Card 的外描边需要留在滚动区内，避免顶部贴边时被视口裁掉。 */}
        <div className="admin-content flex min-w-0 flex-1 flex-col gap-4 overflow-y-auto p-4 pt-1 [&>*]:shrink-0">
          {view === "overview" && (
            <OverviewPage onAccessFailure={onAccessFailure} />
          )}
          {view === "users" && (
            <UsersPage
              onAccessFailure={onAccessFailure}
              onDetailChange={setUserDetail}
            />
          )}
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
