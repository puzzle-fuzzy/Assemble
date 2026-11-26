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
  CloudIcon,
  FilmIcon,
  InfoIcon,
  LogOutIcon,
  LogsIcon,
  MenuIcon,
  MonitorIcon,
  RadioTowerIcon,
  UsersIcon,
} from "lucide-react";
import { lazy, Suspense, useEffect, useRef, useState } from "react";
import type { AccessFailure, AdminSession } from "../lib/api";
import { useAdminTranslation } from "../lib/i18n";
import { AboutDialog } from "./about-dialog";
import { DrivePage } from "./drive-page";
import { LoadingContent } from "./feedback";
import { DevicesPage, LoginsPage, OverviewPage, UsersPage } from "./pages";
import { AdminSystemNotifications } from "./system-notifications";

const MediaPage = lazy(async () => ({
  default: (await import("./media-page")).MediaPage,
}));
const DhtPage = lazy(async () => ({
  default: (await import("./dht-page")).DhtPage,
}));

const navigation = [
  { key: "overview", icon: ChartNoAxesCombinedIcon },
  { key: "users", icon: UsersIcon },
  { key: "devices", icon: MonitorIcon },
  { key: "drive", icon: CloudIcon },
  { key: "logins", icon: LogsIcon },
  { key: "media", icon: FilmIcon },
  { key: "dht", icon: RadioTowerIcon },
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
  const [aboutOpen, setAboutOpen] = useState(false);
  const [storageFocus, setStorageFocus] = useState(0);
  const accountFooter = useRef<HTMLDivElement>(null);
  const mobileMenuTrigger = useRef<HTMLButtonElement>(null);
  const aboutReturnFocus = useRef<HTMLElement | null>(null);
  const pageHeading = useRef<HTMLSpanElement>(null);
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
  const pageName = t(`navigation.${selected.key}`);
  useEffect(() => {
    document.title = t("app.documentTitle", { page: pageName });
  }, [pageName, t]);
  const focusKey = view;
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
    setView(next);
  }

  return (
    // 沿用官方 sidebar-13 的内部骨架；按用户要求移出演示对话框，右侧使用整页空间。
    <SidebarProvider className="h-dvh min-h-0 items-stretch">
      <form ref={logout} method="post" action="/web/logout?next=/admin/" hidden>
        <input type="hidden" name="csrf" value={session.csrfToken} />
      </form>
      <Sidebar collapsible="none" className="hidden min-h-0 shrink-0 md:flex">
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
        <SidebarFooter ref={accountFooter}>
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
                  label: t("navigation.about"),
                  icon: InfoIcon,
                  onSelect: () => {
                    aboutReturnFocus.current =
                      accountFooter.current?.querySelector("button") ?? null;
                    setAboutOpen(true);
                  },
                },
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
      <main className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        <header className="flex h-12 shrink-0 items-center justify-between gap-2">
          <div className="flex min-w-0 items-center gap-2 px-4">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  ref={mobileMenuTrigger}
                  variant="ghost"
                  size="icon"
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
                    onSelect={() => {
                      aboutReturnFocus.current = mobileMenuTrigger.current;
                      setAboutOpen(true);
                    }}
                  >
                    <InfoIcon aria-hidden="true" />
                    {t("navigation.about")}
                  </DropdownMenuItem>
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
          <div className="flex shrink-0 items-center px-4">
            <AdminSystemNotifications
              key={session.account.id}
              session={session}
              onAccessFailure={onAccessFailure}
              onStorage={() => {
                navigate("overview");
                setStorageFocus((value) => value + 1);
              }}
            />
          </div>
        </header>
        {/* Card 的外描边需要留在滚动区内，避免顶部贴边时被视口裁掉。 */}
        <div className="admin-content flex min-h-0 min-w-0 flex-1 flex-col gap-4 overflow-y-auto overscroll-y-contain p-4 pt-1 [&>*]:shrink-0">
          {view === "overview" && (
            <OverviewPage
              key={storageFocus}
              onAccessFailure={onAccessFailure}
              storageFocus={storageFocus}
            />
          )}
          {view === "users" && (
            <UsersPage session={session} onAccessFailure={onAccessFailure} />
          )}
          {view === "devices" && (
            <DevicesPage onAccessFailure={onAccessFailure} />
          )}
          {view === "drive" && <DrivePage onAccessFailure={onAccessFailure} />}
          {view === "logins" && (
            <LoginsPage onAccessFailure={onAccessFailure} />
          )}
          {view === "media" && (
            <Suspense fallback={<LoadingContent />}>
              <MediaPage session={session} onAccessFailure={onAccessFailure} />
            </Suspense>
          )}
          {view === "dht" && (
            <Suspense fallback={<LoadingContent />}>
              <DhtPage session={session} onAccessFailure={onAccessFailure} />
            </Suspense>
          )}
        </div>
      </main>
      <AboutDialog
        open={aboutOpen}
        onOpenChange={setAboutOpen}
        returnFocus={aboutReturnFocus}
      />
    </SidebarProvider>
  );
}
