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
import { LoadingState } from "@assemble/ui/loading-state";
import { AlertCircleIcon, LogInIcon, ShieldXIcon } from "lucide-react";
import type { AccessFailure, RequestFailure } from "../lib/api";
import { useAdminTranslation } from "../lib/i18n";

export function LoadingContent() {
  const { t } = useAdminTranslation();
  return <LoadingState className="min-h-48" label={t("common.loading")} />;
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
        <Button variant="outline" size="default" onClick={retry}>
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
