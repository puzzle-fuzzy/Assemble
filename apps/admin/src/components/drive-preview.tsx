import { Button } from "@assemble/ui/button";
import { FilePreviewContent } from "@assemble/ui/file-preview";
import { isFilePreview } from "@assemble/ui/lib/file-preview";
import { useCallback } from "react";
import {
  type AccessFailure,
  type AdminDriveFile,
  AdminRequestError,
  adminDriveDownloadPath,
  request,
} from "../lib/api";
import { useAdminTranslation } from "../lib/i18n";
import { DetailDialog } from "./detail-dialog";

export function DrivePreviewDialog({
  file,
  onClose,
  onAccessFailure,
}: {
  file: AdminDriveFile;
  onClose: () => void;
  onAccessFailure: (failure: AccessFailure) => void;
}) {
  const { t } = useAdminTranslation();
  const load = useCallback(
    async (signal: AbortSignal) => {
      const path = `/v1/admin/drive/files/${encodeURIComponent(file.id)}/preview`;
      try {
        const preview = await request<unknown>(path, signal);
        if (!isFilePreview(preview)) throw new AdminRequestError("invalid");
        return {
          preview,
          src: ["image", "audio", "video"].includes(preview.kind)
            ? `${path}/content`
            : null,
        };
      } catch (error) {
        if (
          !signal.aborted &&
          error instanceof AdminRequestError &&
          ["signedOut", "forbidden"].includes(error.kind)
        )
          onAccessFailure(error.kind as AccessFailure);
        throw error;
      }
    },
    [file.id, onAccessFailure],
  );
  return (
    <DetailDialog title={file.name} onClose={onClose}>
      <FilePreviewContent
        name={file.name}
        load={load}
        download={
          <Button asChild variant="outline">
            <a href={adminDriveDownloadPath(file.id)} download>
              {t("drive.download")}
            </a>
          </Button>
        }
      />
    </DetailDialog>
  );
}
