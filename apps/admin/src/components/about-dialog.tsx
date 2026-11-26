import { Button } from "@assemble/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@assemble/ui/dialog";
import type { RefObject } from "react";
import packageInfo from "../../package.json";
import { useAdminTranslation } from "../lib/i18n";

export function AboutDialog({
  open,
  onOpenChange,
  returnFocus,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  returnFocus: RefObject<HTMLElement | null>;
}) {
  const { t } = useAdminTranslation();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="sm:max-w-md"
        aria-describedby={undefined}
        onCloseAutoFocus={(event) => {
          if (returnFocus.current?.isConnected) {
            event.preventDefault();
            returnFocus.current.focus();
          }
        }}
      >
        <DialogHeader>
          <DialogTitle>{t("navigation.about")}</DialogTitle>
        </DialogHeader>
        <p className="font-medium">{t("app.title")}</p>
        <dl className="grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-3">
          <dt className="text-muted-foreground">{t("about.version")}</dt>
          <dd>{packageInfo.version}</dd>
          <dt className="text-muted-foreground">{t("about.website")}</dt>
          <dd className="min-w-0">
            <Button
              variant="link"
              className="h-auto min-h-8 justify-start p-0 whitespace-normal break-all"
              asChild
            >
              <a href="https://as.yxswy.com" target="_blank" rel="noreferrer">
                https://as.yxswy.com
              </a>
            </Button>
          </dd>
        </dl>
      </DialogContent>
    </Dialog>
  );
}
