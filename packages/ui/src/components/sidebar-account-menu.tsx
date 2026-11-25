// 复用 sidebar-08 的账户区组合；只接收公开身份与具名动作，不接触会话凭据。
import { Avatar, AvatarFallback, AvatarImage } from "@assemble/ui/avatar";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@assemble/ui/dropdown-menu";
import {
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from "@assemble/ui/sidebar";
import { ChevronsUpDownIcon, type LucideIcon } from "lucide-react";
import { Fragment, useRef } from "react";
import { DisplayPreferencesItems } from "./display-preferences";

export interface SidebarAccountIdentity {
  name: string;
  secondary: string;
  avatarUrl: string | null;
}
export interface SidebarAccountAction {
  label: string;
  icon: LucideIcon;
  onSelect: () => void;
  disabled?: boolean;
  focusPage?: boolean;
  current?: boolean;
}

export function SidebarAccountMenu({
  identity,
  label,
  groups,
  pending = false,
  active = false,
  onNavigateFocus,
}: {
  identity: SidebarAccountIdentity;
  label: string;
  groups: ReadonlyArray<ReadonlyArray<SidebarAccountAction>>;
  pending?: boolean;
  active?: boolean;
  onNavigateFocus?: () => void;
}) {
  const { isMobile } = useSidebar();
  const focusPageOnClose = useRef(false);
  return (
    <SidebarMenu>
      <SidebarMenuItem>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <SidebarMenuButton
              size="lg"
              aria-label={label}
              isActive={active}
              className="data-[state=open]:bg-sidebar-accent data-[state=open]:text-sidebar-accent-foreground"
            >
              <AccountAvatar identity={identity} />
              <IdentityText identity={identity} />
              <ChevronsUpDownIcon aria-hidden="true" className="ml-auto" />
            </SidebarMenuButton>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            className="w-(--radix-dropdown-menu-trigger-width) min-w-56 rounded-lg"
            side={isMobile ? "bottom" : "right"}
            align="end"
            sideOffset={4}
            collisionPadding={8}
            onCloseAutoFocus={(event) => {
              if (focusPageOnClose.current && onNavigateFocus) {
                event.preventDefault();
                focusPageOnClose.current = false;
                onNavigateFocus();
              }
            }}
          >
            <DropdownMenuLabel className="p-0 font-normal text-popover-foreground">
              <div className="flex items-center gap-2 px-1 py-1.5 text-left text-sm">
                <AccountAvatar identity={identity} />
                <IdentityText identity={identity} />
              </div>
            </DropdownMenuLabel>
            <DropdownMenuSeparator />
            <DisplayPreferencesItems />
            <DropdownMenuSeparator />
            {groups
              .filter((group) => group.length > 0)
              .map((group, index) => (
                <Fragment key={group[0].label}>
                  {index > 0 ? <DropdownMenuSeparator /> : null}
                  <DropdownMenuGroup>
                    {group.map(
                      ({
                        label: actionLabel,
                        icon: Icon,
                        onSelect,
                        disabled,
                        focusPage,
                        current,
                      }) => (
                        <DropdownMenuItem
                          key={actionLabel}
                          disabled={pending || disabled}
                          aria-current={current ? "page" : undefined}
                          onSelect={() => {
                            focusPageOnClose.current = focusPage ?? false;
                            onSelect();
                          }}
                        >
                          <Icon aria-hidden="true" />
                          {actionLabel}
                        </DropdownMenuItem>
                      ),
                    )}
                  </DropdownMenuGroup>
                </Fragment>
              ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </SidebarMenuItem>
    </SidebarMenu>
  );
}

function AccountAvatar({ identity }: { identity: SidebarAccountIdentity }) {
  return (
    <Avatar className="size-8 rounded-lg after:rounded-lg">
      {identity.avatarUrl ? (
        <AvatarImage
          src={identity.avatarUrl}
          alt=""
          referrerPolicy="no-referrer"
          className="rounded-lg"
        />
      ) : null}
      <AvatarFallback className="rounded-lg">
        {Array.from(identity.name)[0]?.toUpperCase() || "A"}
      </AvatarFallback>
    </Avatar>
  );
}

function IdentityText({ identity }: { identity: SidebarAccountIdentity }) {
  return (
    <div className="grid min-w-0 flex-1 text-left text-sm leading-tight">
      <span className="truncate font-medium">{identity.name}</span>
      <span className="truncate text-xs">{identity.secondary}</span>
    </div>
  );
}
