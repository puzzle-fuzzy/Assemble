import { Button } from "@assemble/ui/button";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@assemble/ui/empty";
import { Skeleton } from "@assemble/ui/skeleton";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  FilmIcon,
  RotateCwIcon,
} from "lucide-react";
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
    <section aria-busy="true" aria-label={t("loading")}>
      <p className="sr-only" role="status">
        {t("loading")}
      </p>
      {wall ? (
        <div className="poster-grid">
          {["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"].map(
            (key) => (
              <div className="flex flex-col gap-3" key={key}>
                <Skeleton className="aspect-[2/3] w-full rounded-xl" />
                <Skeleton className="h-4 w-3/4" />
                <Skeleton className="h-3 w-1/3" />
              </div>
            ),
          )}
        </div>
      ) : (
        <div className="flex flex-col gap-5">
          <Skeleton className="h-10 w-2/3" />
          <Skeleton className="h-5 w-1/2" />
          <Skeleton className="h-64 w-full" />
        </div>
      )}
    </section>
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
        <EmptyMedia variant="icon">
          <FilmIcon />
        </EmptyMedia>
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
    <Button asChild variant="ghost">
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
}: {
  page: number;
  total: number;
  pageSize: number;
  onPage: (page: number) => void;
}) {
  const { t } = useSiteTranslation();
  const pages = Math.max(1, Math.ceil(total / pageSize));
  return (
    <nav className="media-pagination" aria-label={t("pagination")}>
      <p className="text-sm text-muted-foreground" aria-live="polite">
        {t("results", { count: total, page, pages })}
      </p>
      <div className="flex gap-2">
        <Button
          variant="outline"
          disabled={page <= 1}
          onClick={() => onPage(page - 1)}
        >
          <ArrowLeftIcon data-icon="inline-start" />
          {t("previous")}
        </Button>
        <Button
          variant="outline"
          disabled={page >= pages}
          onClick={() => onPage(page + 1)}
        >
          {t("next")}
          <ArrowRightIcon data-icon="inline-end" />
        </Button>
      </div>
    </nav>
  );
}
