import { Button } from "@assemble/ui/button";
import { useState } from "react";
import { useActivityTranslation } from "../lib/activity-i18n";
import type { AccessFailure, AdminDriveFile } from "../lib/api";
import { RecordedTime } from "../lib/text";
import { useResource } from "../lib/use-resource";
import { DetailDialog, DetailFields } from "./detail-dialog";
import { DrivePreviewDialog } from "./drive-preview";
import { LoadingContent, RequestError } from "./feedback";
import { storageBytes } from "./storage-overview";

export function ActivityFileFocus({
  id,
  onClose,
  onAccessFailure,
}: {
  id: string;
  onClose: () => void;
  onAccessFailure: (failure: AccessFailure) => void;
}) {
  const { t, i18n } = useActivityTranslation();
  const { resource, retry } = useResource<AdminDriveFile>(
    `/v1/admin/drive/files/${encodeURIComponent(id)}`,
    onAccessFailure,
  );
  const [preview, setPreview] = useState(false);
  return (
    <>
      <DetailDialog title={t("fileDetails")} onClose={onClose}>
        {resource.phase === "loading" ? (
          <LoadingContent />
        ) : resource.phase === "error" ? (
          <RequestError failure={resource.failure} retry={retry} />
        ) : (
          <>
            <DetailFields
              fields={[
                { label: t("name"), value: resource.value.name },
                { label: t("id"), value: resource.value.id },
                { label: t("owner"), value: resource.value.accountDisplayName },
                {
                  label: t("size"),
                  value: storageBytes(
                    resource.value.size,
                    i18n.resolvedLanguage || i18n.language,
                  ),
                },
                {
                  label: t("mime"),
                  value: resource.value.mimeType ?? t("unknown"),
                },
                {
                  label: t("created"),
                  value: <RecordedTime value={resource.value.createdAt} />,
                },
              ]}
            />
            <Button variant="outline" onClick={() => setPreview(true)}>
              {t("preview")}
            </Button>
          </>
        )}
      </DetailDialog>
      {preview && resource.phase === "ready" ? (
        <DrivePreviewDialog
          file={resource.value}
          onClose={() => setPreview(false)}
          onAccessFailure={onAccessFailure}
        />
      ) : null}
    </>
  );
}
