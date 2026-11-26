import { Button } from "@assemble/ui/button";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@assemble/ui/empty";
import { LoadingState } from "@assemble/ui/loading-state";
import { ArrowLeftIcon, ArrowRightIcon, RotateCwIcon } from "lucide-react";
import type { ComponentProps } from "react";
import { useSiteTranslation } from "../lib/i18n";
import { loginPath, navigate } from "../lib/navigation";

export function SiteLink({
  href = "/movies/",
  onClick,
  ...props
}: ComponentProps<"a">) {
  return (
    <a
      href={href}
      {...props}
      onClick={(event) => {
        onClick?.(event);
        if (
          !event.defaultPrevented &&
          event.button === 0 &&
          !event.metaKey &&
          !event.ctrlKey &&
          !event.shiftKey &&
          !event.altKey &&
          !props.target &&
          href.startsWith("/movies/")
        ) {
          event.preventDefault();
          navigate(href);
        }
      }}
    />
  );
}

export function LoadingContent({ wall = false }: { wall?: boolean }) {
  const { t } = useSiteTranslation();
  return (
    <LoadingState
      className={wall ? "min-h-[50svh]" : "min-h-64"}
      label={t("loading")}
    />
  );
}

export function EmptyContentView({
  title,
  description,
  children,
}: {
  title: string;
  description?: string;
  children?: React.ReactNode;
}) {
  return (
    <Empty>
      <EmptyHeader>
        <EmptyTitle>{title}</EmptyTitle>
        {description && <EmptyDescription>{description}</EmptyDescription>}
      </EmptyHeader>
      {children && <EmptyContent>{children}</EmptyContent>}
    </Empty>
  );
}

export function RequestError({
  status,
  retry,
}: {
  status: number | null;
  retry: () => void;
}) {
  const { t } = useSiteTranslation();
  const denied = status === 401 || status === 403;
  return (
    <section role="alert">
      <EmptyContentView
        title={t(status === 404 ? "notFound" : denied ? "denied" : "error")}
        description={t(
          status === 404 ? "notFoundHelp" : denied ? "deniedHelp" : "errorHelp",
        )}
      >
        {denied && (
          <Button asChild>
            <a href={status === 401 ? loginPath : "/web/account"}>
              {t(status === 401 ? "signIn" : "account")}
            </a>
          </Button>
        )}
        <Button variant="outline" onClick={retry}>
          <RotateCwIcon data-icon="inline-start" />
          {t("retry")}
        </Button>
      </EmptyContentView>
    </section>
  );
}

export function BackLink({ href = "/movies/" }: { href?: string }) {
  const { t } = useSiteTranslation();
  return (
    <Button asChild variant="ghost" className="w-fit">
      <SiteLink href={href}>
        <ArrowLeftIcon data-icon="inline-start" />
        {t("back")}
      </SiteLink>
    </Button>
  );
}

export function Pagination({
  page,
  total,
  pageSize,
  onPage,
  compact = false,
  pending = false,
  label,
}: {
  page: number;
  total: number;
  pageSize: number;
  onPage: (page: number) => void;
  compact?: boolean;
  pending?: boolean;
  label?: string;
}) {
  const { t } = useSiteTranslation();
  const pages = Math.max(1, Math.ceil(total / pageSize));
  if (pages === 1) return null;
  return (
    <nav className="media-pagination" aria-label={label ?? t("pagination")}>
      <p className="text-sm text-muted-foreground" aria-live="polite">
        {t(compact ? "pagePosition" : "results", { count: total, page, pages })}
      </p>
      <div className="flex gap-2">
        <Button
          variant="outline"
          disabled={page <= 1}
          aria-disabled={pending || page <= 1}
          onClick={() => {
            if (!pending) onPage(page - 1);
          }}
        >
          <ArrowLeftIcon data-icon="inline-start" />
          {t("previous")}
        </Button>
        <Button
          variant="outline"
          disabled={page >= pages}
          aria-disabled={pending || page >= pages}
          onClick={() => {
            if (!pending) onPage(page + 1);
          }}
        >
          {t("next")}
          <ArrowRightIcon data-icon="inline-end" />
        </Button>
      </div>
    </nav>
  );
}
