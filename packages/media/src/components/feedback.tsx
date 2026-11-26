import { Alert, AlertDescription, AlertTitle } from "@assemble/ui/alert";
import { Button } from "@assemble/ui/button";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@assemble/ui/empty";
import { LoadingState } from "@assemble/ui/loading-state";
import { FilmIcon, RefreshCwIcon } from "lucide-react";
import { mediaErrorCode } from "../hooks/use-media-task";
import { useMediaTranslation } from "../i18n";

export function MediaError({
  error,
  retry,
}: {
  error: unknown;
  retry?: () => void;
}) {
  const { t } = useMediaTranslation();
  if (!error) return null;
  const code = mediaErrorCode(error);
  return (
    <Alert variant="destructive">
      <AlertTitle>
        {t(`error.${code}`, { defaultValue: t("error.generic") })}
      </AlertTitle>
      {retry && (
        <AlertDescription>
          <Button variant="outline" size="default" onClick={retry}>
            <RefreshCwIcon data-icon="inline-start" />
            {t("common.retry")}
          </Button>
        </AlertDescription>
      )}
    </Alert>
  );
}
export function MediaLoading() {
  const { t } = useMediaTranslation();
  return <LoadingState className="min-h-48" label={t("common.loading")} />;
}
export function MediaEmpty({
  title,
  description,
}: {
  title?: string;
  description?: string;
}) {
  const { t } = useMediaTranslation();
  return (
    <Empty>
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <FilmIcon />
        </EmptyMedia>
        <EmptyTitle>{title ?? t("common.empty")}</EmptyTitle>
        {description && <EmptyDescription>{description}</EmptyDescription>}
      </EmptyHeader>
    </Empty>
  );
}
export function MediaPagination({
  page,
  total,
  pageSize,
  onPage,
  pending,
}: {
  page: number;
  total: number;
  pageSize: number;
  onPage: (value: number) => void;
  pending?: boolean;
}) {
  const { t } = useMediaTranslation();
  const pages = Math.max(1, Math.ceil(total / pageSize));
  return (
    <nav
      aria-label={t("list.showing")}
      className="flex flex-wrap items-center justify-between gap-3"
    >
      <p className="text-sm text-muted-foreground">
        {t("common.page", { total, page, pages })}
      </p>
      <div className="flex gap-2">
        <Button
          variant="outline"
          size="default"
          disabled={pending || page <= 1}
          onClick={() => onPage(page - 1)}
        >
          {t("common.previous")}
        </Button>
        <Button
          variant="outline"
          size="default"
          disabled={pending || page >= pages}
          onClick={() => onPage(page + 1)}
        >
          {t("common.next")}
        </Button>
      </div>
    </nav>
  );
}
