import { Alert, AlertDescription, AlertTitle } from "@assemble/ui/alert";
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
import { AlertCircleIcon, LogInIcon, ShieldXIcon } from "lucide-react";
import type { AccessFailure, RequestFailure } from "../lib/api";
import { useAdminTranslation } from "../lib/i18n";

export function LoadingContent() {
  const { t } = useAdminTranslation();
  return (
    <div aria-busy="true" className="flex flex-col gap-4">
      <span className="sr-only">{t("common.loading")}</span>
      <Skeleton className="h-8 w-40" />
      <Skeleton className="h-20 w-full" />
      <Skeleton className="h-20 w-full" />
    </div>
  );
}

export function RequestError({
  failure,
  retry,
}: {
  failure: RequestFailure;
  retry: () => void;
}) {
  const { t } = useAdminTranslation();
  return (
    <Alert variant="destructive">
      <AlertCircleIcon aria-hidden="true" />
      <AlertTitle>
        {t(
          failure === "notFound"
            ? "error.notFoundTitle"
            : "error.unavailableTitle",
        )}
      </AlertTitle>
      <AlertDescription>
        <p>
          {t(
            failure === "notFound"
              ? "error.notFoundDescription"
              : "error.unavailableDescription",
          )}
        </p>
        <Button variant="outline" size="sm" onClick={retry}>
          {t("common.retry")}
        </Button>
      </AlertDescription>
    </Alert>
  );
}

export function NoResults({
  title,
  description,
}: {
  title: string;
  description: string;
}) {
  return (
    <Empty>
      <EmptyHeader>
        <EmptyTitle>{title}</EmptyTitle>
        <EmptyDescription>{description}</EmptyDescription>
      </EmptyHeader>
    </Empty>
  );
}

export function AccessRequired({ failure }: { failure: AccessFailure }) {
  const { t } = useAdminTranslation();
  const signedOut = failure === "signedOut";
  return (
    <Empty>
      <EmptyHeader>
        <EmptyMedia variant="icon">
          {signedOut ? (
            <LogInIcon aria-hidden="true" />
          ) : (
            <ShieldXIcon aria-hidden="true" />
          )}
        </EmptyMedia>
        <EmptyTitle>
          {t(signedOut ? "access.signedOutTitle" : "access.forbiddenTitle")}
        </EmptyTitle>
        <EmptyDescription>
          {t(
            signedOut
              ? "access.signedOutDescription"
              : "access.forbiddenDescription",
          )}
        </EmptyDescription>
      </EmptyHeader>
      <EmptyContent>
        <Button asChild>
          <a href={signedOut ? "/web/login?next=/admin/" : "/web/account"}>
            {t(signedOut ? "access.signIn" : "access.openAccount")}
          </a>
        </Button>
      </EmptyContent>
    </Empty>
  );
}
