import { Button } from "@assemble/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@assemble/ui/dialog";
import { Field, FieldError, FieldGroup, FieldLabel } from "@assemble/ui/field";
import { Input } from "@assemble/ui/input";
import { useEffect, useId, useRef, useState } from "react";
import { type Bridge, toBridgeError } from "@/lib/bridge";
import { useDriveTranslation } from "@/lib/drive-i18n";
import type { DriveFileView } from "@/lib/types";

/** 与协议的 clean_file_name 相同；最终校验仍由 Rust 与服务端负责。 */
export function cleanDriveFileName(value: string): string | null {
  const name = value.trim();
  return name.length > 0 &&
    name.length <= 255 &&
    name !== "." &&
    name !== ".." &&
    !/[/\\]/u.test(name) &&
    ![...name].some((character) => {
      const code = character.codePointAt(0) ?? 0;
      return code < 32 || (code >= 127 && code <= 159);
    })
    ? name
    : null;
}

export function DriveRenameAction({
  source,
  file,
  disabled = false,
}: {
  source: Pick<Bridge, "driveRename">;
  file: DriveFileView;
  disabled?: boolean;
}) {
  const { t } = useDriveTranslation();
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState(false);
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!pending) setOpen(next);
      }}
    >
      <DialogTrigger asChild>
        <Button variant="outline" disabled={disabled}>
          {t("rename")}
        </Button>
      </DialogTrigger>
      {open ? (
        <DriveRenameForm
          key={file.id}
          source={source}
          file={file}
          pending={pending}
          onPending={setPending}
          onClose={() => setOpen(false)}
        />
      ) : null}
    </Dialog>
  );
}

function DriveRenameForm({
  source,
  file,
  pending,
  onPending,
  onClose,
}: {
  source: Pick<Bridge, "driveRename">;
  file: DriveFileView;
  pending: boolean;
  onPending: (pending: boolean) => void;
  onClose: () => void;
}) {
  const { t } = useDriveTranslation();
  const id = useId();
  const input = useRef<HTMLInputElement>(null);
  const submitting = useRef(false);
  const [name, setName] = useState(file.name);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (error && !pending) input.current?.focus();
  }, [error, pending]);

  async function save() {
    if (submitting.current) return;
    const cleaned = cleanDriveFileName(name);
    if (cleaned === null) {
      setError("invalid");
      input.current?.focus();
      return;
    }
    submitting.current = true;
    onPending(true);
    setError(null);
    try {
      await source.driveRename(file.id, cleaned);
      onClose();
    } catch (cause: unknown) {
      const code = toBridgeError(cause).code;
      setError(
        code === "INVALID_REQUEST"
          ? "invalid"
          : code === "NOT_FOUND"
            ? "missing"
            : code === "UNAUTHORIZED"
              ? "signIn"
              : "failed",
      );
      input.current?.focus();
    } finally {
      submitting.current = false;
      onPending(false);
    }
  }

  return (
    <DialogContent
      aria-describedby={undefined}
      onOpenAutoFocus={(event) => {
        event.preventDefault();
        input.current?.focus();
        input.current?.select();
      }}
    >
      <DialogHeader>
        <DialogTitle>{t("rename")}</DialogTitle>
      </DialogHeader>
      <form
        noValidate
        className="flex flex-col gap-4"
        aria-busy={pending}
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <FieldGroup>
          <Field data-invalid={error === "invalid" || undefined}>
            <FieldLabel className="sr-only" htmlFor={`${id}-name`}>
              {t("name")}
            </FieldLabel>
            <Input
              ref={input}
              id={`${id}-name`}
              name="fileName"
              required
              autoComplete="off"
              placeholder={t("name")}
              disabled={pending}
              value={name}
              aria-invalid={error === "invalid"}
              aria-describedby={error ? `${id}-error` : undefined}
              onChange={(event) => {
                setName(event.target.value);
                setError(null);
              }}
            />
            {error ? (
              <FieldError id={`${id}-error`}>{t(`errors.${error}`)}</FieldError>
            ) : null}
          </Field>
        </FieldGroup>
        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            disabled={pending}
            onClick={onClose}
          >
            {t("cancel")}
          </Button>
          <Button type="submit" disabled={pending}>
            {t(pending ? "saving" : "save")}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  );
}
