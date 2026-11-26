import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@assemble/ui/dialog";
import { type ReactNode, useRef } from "react";

export function DetailDialog({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
}) {
  const returnFocus = useRef<HTMLElement | null>(null);
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        className="max-h-[90svh] overflow-y-auto sm:max-w-5xl"
        aria-describedby={undefined}
        onOpenAutoFocus={() => {
          returnFocus.current =
            document.activeElement instanceof HTMLElement
              ? document.activeElement
              : null;
        }}
        onCloseAutoFocus={(event) => {
          if (returnFocus.current?.isConnected) {
            event.preventDefault();
            returnFocus.current.focus();
          }
        }}
      >
        <DialogHeader className="pr-8">
          <DialogTitle>{title}</DialogTitle>
        </DialogHeader>
        {children}
      </DialogContent>
    </Dialog>
  );
}

export function DetailFields({
  fields,
}: {
  fields: Array<{ label: string; value: ReactNode }>;
}) {
  return (
    <dl className="grid gap-x-8 gap-y-4 sm:grid-cols-2">
      {fields.map(({ label, value }) => (
        <div key={label} className="flex min-w-0 flex-col gap-1">
          <dt className="text-sm text-muted-foreground">{label}</dt>
          <dd className="break-words text-sm wrap-anywhere">{value}</dd>
        </div>
      ))}
    </dl>
  );
}
