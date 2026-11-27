// 云盘上传沿用现有传输行；按钮只传任务标识，不接收文件位置。
import { FolderOpen, Pause, Play } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { type Bridge, toBridgeError } from "@/lib/bridge";
import { uiText } from "@/lib/i18n";
import type { TransferView } from "@/lib/types";

export function UploadControls({
  source,
  transfer,
  onError,
  disabled = false,
  withCancel = false,
}: {
  source: Bridge;
  transfer: TransferView;
  onError: (code: string | null) => void;
  disabled?: boolean;
  withCancel?: boolean;
}) {
  const [pending, setPending] = useState(false);
  if (
    !transfer.resumable ||
    !["queued", "inProgress", "paused"].includes(transfer.phase)
  )
    return null;
  const paused = transfer.phase === "paused";
  const label = uiText(paused ? "resumeUploadFor" : "pauseUploadFor", {
    name: transfer.name,
  });
  async function act(work: () => Promise<void>) {
    setPending(true);
    onError(null);
    try {
      await work();
    } catch (error) {
      onError(toBridgeError(error).code);
    } finally {
      setPending(false);
    }
  }
  return (
    <>
      {paused &&
      transfer.failure === "fileUnreadable" &&
      transfer.direction === "out" ? (
        <Button
          size="icon"
          variant="outline"
          disabled={disabled || pending}
          aria-label={uiText("relocateTransferFor", { name: transfer.name })}
          title={uiText("relocateTransferFor", { name: transfer.name })}
          onClick={() => void act(() => source.relocateTransfer(transfer.id))}
        >
          <FolderOpen aria-hidden />
        </Button>
      ) : null}
      <Button
        size="icon"
        variant="outline"
        disabled={disabled || pending}
        aria-label={label}
        title={label}
        onClick={() =>
          void act(() =>
            paused
              ? source.resumeTransfer(transfer.id)
              : source.pauseTransfer(transfer.id),
          )
        }
      >
        {paused ? <Play aria-hidden /> : <Pause aria-hidden />}
      </Button>
      {withCancel ? (
        <Button
          variant="outline"
          disabled={disabled || pending}
          aria-label={uiText("cancelUploadFor", { name: transfer.name })}
          onClick={() => void act(() => source.cancelTransfer(transfer.id))}
        >
          {uiText("cancel")}
        </Button>
      ) : null}
    </>
  );
}
