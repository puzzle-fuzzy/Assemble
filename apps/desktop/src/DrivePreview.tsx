import { Button } from "@assemble/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@assemble/ui/dialog";
import { FilePreviewContent } from "@assemble/ui/file-preview";
import { useUiTranslation } from "@assemble/ui/lib/i18n";
import { useCallback, useState } from "react";
import { toast } from "sonner";
import { type Bridge, toBridgeError } from "./lib/bridge";
import { uiText } from "./lib/i18n";
import { messageFor } from "./lib/messages";
import type { DriveFileView } from "./lib/types";

export function DrivePreviewAction({
  source,
  file,
  disabled,
}: {
  source: Bridge;
  file: DriveFileView;
  disabled: boolean;
}) {
  const { t } = useUiTranslation();
  const [open, setOpen] = useState(false);
  const load = useCallback(
    (signal: AbortSignal) => source.drivePreview(file.id, signal),
    [source, file.id],
  );
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button variant="outline" disabled={disabled}>
          {t("filePreview.open")}
        </Button>
      </DialogTrigger>
      <DialogContent
        className="max-h-[90svh] overflow-y-auto sm:max-w-4xl"
        aria-describedby={undefined}
      >
        <DialogHeader className="pr-8">
          <DialogTitle className="break-all">{file.name}</DialogTitle>
        </DialogHeader>
        {open && (
          <FilePreviewContent
            name={file.name}
            load={load}
            download={
              <Button
                variant="outline"
                onClick={() => {
                  void source
                    .driveDownload(file.id)
                    .then(() => setOpen(false))
                    .catch((error: unknown) =>
                      toast.error(messageFor(toBridgeError(error))),
                    );
                }}
              >
                {uiText("download")}
              </Button>
            }
          />
        )}
      </DialogContent>
    </Dialog>
  );
}
