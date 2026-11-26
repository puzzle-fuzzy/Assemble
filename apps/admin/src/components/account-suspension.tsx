import { Alert, AlertTitle } from "@assemble/ui/alert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@assemble/ui/alert-dialog";
import { Button } from "@assemble/ui/button";
import { useEffect, useRef, useState } from "react";
import {
  type AccessFailure,
  AdminRequestError,
  type AdminSession,
  type AdminUser,
  type RequestFailure,
  request,
} from "../lib/api";
import { useOperationsTranslation } from "../lib/operations-i18n";

export function AccountStatus({ suspended }: { suspended: boolean }) {
  const { t } = useOperationsTranslation();
  return <span>{t(suspended ? "account.suspended" : "account.active")}</span>;
}

export function AccountSuspension({
  user,
  session,
  onUpdated,
  onAccessFailure,
}: {
  user: AdminUser;
  session: AdminSession;
  onUpdated: (user: AdminUser) => void;
  onAccessFailure: (failure: AccessFailure) => void;
}) {
  const { t } = useOperationsTranslation();
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<RequestFailure | null>(null);
  const active = useRef<AbortController | null>(null);
  useEffect(() => () => active.current?.abort(), []);
  if (user.id === session.account.id) return null;
  const action = user.suspended ? "resume" : "suspend";
  async function confirm() {
    if (active.current) return;
    const controller = new AbortController();
    active.current = controller;
    setPending(true);
    setFailure(null);
    try {
      const updated = await request<AdminUser>(
        `/v1/admin/users/${encodeURIComponent(user.id)}/${action}`,
        controller.signal,
        { csrfToken: session.csrfToken },
      );
      if (!controller.signal.aborted) {
        onUpdated(updated);
        setOpen(false);
      }
    } catch (error: unknown) {
      if (controller.signal.aborted) return;
      const kind =
        error instanceof AdminRequestError ? error.kind : "unavailable";
      if (kind === "signedOut" || kind === "forbidden") onAccessFailure(kind);
      else setFailure(kind);
    } finally {
      if (active.current === controller) active.current = null;
      if (!controller.signal.aborted) setPending(false);
    }
  }
  return (
    <AlertDialog
      open={open}
      onOpenChange={(value) => {
        if (pending) return;
        setFailure(null);
        setOpen(value);
      }}
    >
      <AlertDialogTrigger asChild>
        <Button
          variant={user.suspended ? "outline" : "destructive"}
          size="default"
        >
          {t(`account.${action}`)}
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent
        onEscapeKeyDown={(event) => {
          if (pending) event.preventDefault();
        }}
      >
        <AlertDialogHeader>
          <AlertDialogTitle>
            {t(`account.${action}Title`, { name: user.displayName })}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {t(`account.${action}Description`)}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {failure && (
          <Alert variant="destructive">
            <AlertTitle>
              {t(`account.error.${failure}`, {
                defaultValue: t("account.error.unavailable"),
              })}
            </AlertTitle>
          </Alert>
        )}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>
            {t("account.cancel")}
          </AlertDialogCancel>
          <AlertDialogAction
            variant={user.suspended ? "default" : "destructive"}
            disabled={pending}
            onClick={(event) => {
              event.preventDefault();
              void confirm();
            }}
          >
            {t(pending ? "account.applying" : `account.${action}`)}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
