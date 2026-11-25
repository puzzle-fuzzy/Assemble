// 保留 sidebar-09 的外层收折、内层图标栏与列表栏；窄桌面窗口仍能看见模块入口。
import {
  ArrowLeftRight,
  CalendarClock,
  Cloud,
  PanelsTopLeft,
  Settings2,
} from "lucide-react";
import type { ReactNode } from "react";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@/components/ui/empty";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarHeader,
  SidebarInput,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from "@/components/ui/sidebar";
import { cn } from "@/lib/utils";

export const DESKTOP_MODULES = [
  { id: "transfers", label: "文件传输", icon: ArrowLeftRight },
  { id: "drive", label: "云盘", icon: Cloud },
  { id: "planner", label: "日程与提醒", icon: CalendarClock },
  { id: "settings", label: "本机设置", icon: Settings2 },
] as const;
export type DesktopModule = (typeof DESKTOP_MODULES)[number]["id"];
export interface DesktopListItem {
  id: string;
  title: string;
  meta: string;
  description: string;
  group?: string;
}

export function DesktopSidebar({
  section,
  onNavigate,
  account,
  items,
  selected,
  onSelect,
  query,
  onQuery,
  signedIn,
  loading,
  draft,
}: {
  section: DesktopModule;
  onNavigate: (section: DesktopModule) => void;
  account: ReactNode;
  items: DesktopListItem[];
  selected: string | null;
  onSelect: (id: string) => void;
  query: string;
  onQuery: (query: string) => void;
  signedIn: boolean;
  loading: boolean;
  draft: boolean;
}) {
  const { isMobile, open, setOpen, openMobile, setOpenMobile } = useSidebar();
  const expanded = isMobile ? openMobile : open;
  const title = DESKTOP_MODULES.find((item) => item.id === section)?.label;
  function navigate(next: DesktopModule) {
    onNavigate(next);
    if (isMobile) setOpenMobile(true);
    else setOpen(true);
  }
  return (
    <Sidebar
      collapsible={isMobile ? "none" : "icon"}
      className={cn(
        "overflow-hidden *:data-[sidebar=sidebar]:flex-row",
        isMobile && "flex-row! shrink-0 border-r",
      )}
      style={
        isMobile
          ? {
              width: expanded
                ? "350px"
                : "calc(var(--sidebar-width-icon) + 1px)",
            }
          : undefined
      }
      aria-label="功能侧栏"
      role="complementary"
    >
      <Sidebar
        collapsible="none"
        className="w-[calc(var(--sidebar-width-icon)+1px)]! shrink-0 border-r"
      >
        <SidebarHeader>
          <SidebarMenu>
            <SidebarMenuItem>
              <SidebarMenuButton
                size="lg"
                className="h-8 p-0"
                aria-label="Assemble · 文件传输"
                onClick={() => navigate("transfers")}
              >
                <div className="flex aspect-square size-8 shrink-0 items-center justify-center rounded-lg bg-sidebar-primary text-sidebar-primary-foreground">
                  <PanelsTopLeft aria-hidden="true" className="size-4" />
                </div>
                <span className="sr-only">Assemble</span>
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
        </SidebarHeader>
        <SidebarContent>
          <SidebarGroup>
            <SidebarGroupContent>
              <nav aria-label="主功能">
                <SidebarMenu>
                  {DESKTOP_MODULES.map(({ id, label, icon: Icon }) => (
                    <SidebarMenuItem key={id}>
                      <SidebarMenuButton
                        tooltip={{ children: label, hidden: false }}
                        onClick={() => navigate(id)}
                        isActive={section === id}
                        aria-label={label}
                        aria-current={section === id ? "page" : undefined}
                        aria-controls={`view-${id}`}
                        className="px-2"
                      >
                        <Icon aria-hidden="true" />
                        <span className="sr-only">{label}</span>
                      </SidebarMenuButton>
                    </SidebarMenuItem>
                  ))}
                </SidebarMenu>
              </nav>
            </SidebarGroupContent>
          </SidebarGroup>
        </SidebarContent>
        <SidebarFooter>{account}</SidebarFooter>
      </Sidebar>
      {expanded ? (
        <Sidebar
          collapsible="none"
          className="min-w-0 flex-1"
          aria-label={`${title}列表`}
        >
          <SidebarHeader className="gap-3.5 border-b p-4">
            <div className="flex w-full items-center justify-between gap-2">
              <h2 className="text-base font-medium text-foreground">{title}</h2>
              <span className="text-xs text-muted-foreground">
                {items.length} 项
              </span>
            </div>
            <SidebarInput
              aria-label={`搜索${title}`}
              placeholder="搜索…"
              value={query}
              onChange={(event) => onQuery(event.target.value)}
            />
          </SidebarHeader>
          <SidebarContent>
            <SidebarGroup className="px-0">
              <SidebarGroupContent>
                {draft && section === "planner" ? (
                  <ListItem
                    item={{
                      id: "draft",
                      title: "未保存的草稿",
                      meta: "正在编辑",
                      description: "继续填写日程或提醒",
                    }}
                    selected={selected === "draft"}
                    onSelect={onSelect}
                  />
                ) : null}
                {items.map((item, index) => (
                  <div key={item.id}>
                    {item.group && item.group !== items[index - 1]?.group ? (
                      <h3 className="px-4 py-2 text-xs text-muted-foreground">
                        {item.group}
                      </h3>
                    ) : null}
                    <ListItem
                      item={item}
                      selected={selected === item.id}
                      onSelect={onSelect}
                    />
                  </div>
                ))}
                {items.length === 0 ? (
                  <Empty className="min-w-0 px-4 py-8">
                    <EmptyHeader>
                      <EmptyTitle>
                        {loading
                          ? "正在读取…"
                          : query
                            ? "没有匹配的项目"
                            : "暂无项目"}
                      </EmptyTitle>
                      <EmptyDescription>
                        {!signedIn && section !== "settings"
                          ? "登录后查看账户内容。"
                          : query
                            ? "换个关键词试试。"
                            : "新建的内容会出现在这里。"}
                      </EmptyDescription>
                    </EmptyHeader>
                  </Empty>
                ) : null}
              </SidebarGroupContent>
            </SidebarGroup>
          </SidebarContent>
        </Sidebar>
      ) : null}
    </Sidebar>
  );
}

function ListItem({
  item,
  selected,
  onSelect,
}: {
  item: DesktopListItem;
  selected: boolean;
  onSelect: (id: string) => void;
}) {
  return (
    <SidebarMenu className="gap-0">
      <SidebarMenuItem>
        <SidebarMenuButton
          data-item-id={item.id}
          aria-label={`选择${item.title}`}
          aria-current={selected ? "true" : undefined}
          isActive={selected}
          onClick={() => onSelect(item.id)}
          className="h-auto min-h-24 flex-col items-start gap-2 rounded-none border-b p-4 text-sm leading-tight whitespace-normal"
        >
          <span className="flex w-full min-w-0 items-start gap-2">
            <span className="min-w-0 flex-1 truncate font-medium">
              {item.title}
            </span>
            <span className="shrink-0 text-xs text-muted-foreground">
              {item.meta}
            </span>
          </span>
          <span className="line-clamp-2 w-full text-xs break-words whitespace-break-spaces! text-muted-foreground">
            {item.description}
          </span>
        </SidebarMenuButton>
      </SidebarMenuItem>
    </SidebarMenu>
  );
}
