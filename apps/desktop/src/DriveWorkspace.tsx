import { Button } from "@assemble/ui/button";
import {
  DriveTreeBrowser,
  type DriveTreeLocation,
} from "@assemble/ui/drive-tree-browser";
import {
  driveTreeError,
  useDriveTreeTranslation,
} from "@assemble/ui/lib/drive-tree-i18n";
import { LoadingState } from "@assemble/ui/loading-state";
import { type ReactNode, useEffect, useState } from "react";
import { toast } from "sonner";
import { DrivePreviewAction } from "./DrivePreview";
import type { Bridge } from "./lib/bridge";
import { formatStoredDate } from "./lib/i18n";
import { formatSize, formatSpace } from "./lib/transfer-text";
import type { Snapshot } from "./lib/types";

/** 只有新根入口明确不存在才回退平铺；其它错误不能当成旧服务。 */
export function DriveWorkspace({
  source,
  snapshot,
  legacy,
  onLocationChange,
  progress,
}: {
  source: Bridge;
  snapshot: Snapshot | null;
  legacy: ReactNode;
  onLocationChange: (location: DriveTreeLocation | null) => void;
  progress?: ReactNode;
}) {
  const { t } = useDriveTreeTranslation();
  const [mode, setMode] = useState<"checking" | "tree" | "legacy" | "failed">(
    "checking",
  );
  const [revision, setRevision] = useState(0);
  const account =
    snapshot?.session.phase === "signedIn" ? snapshot.session.account.id : null;
  useEffect(() => {
    void revision;
    if (!account) return;
    const cancel = new AbortController();
    setMode("checking");
    void source.driveTree
      .list({ parentId: "root", q: "", page: 1, pageSize: 10 }, cancel.signal)
      .then(
        () => {
          if (!cancel.signal.aborted) setMode("tree");
        },
        (error) => {
          if (!cancel.signal.aborted)
            setMode(
              typeof error === "object" &&
                error !== null &&
                "code" in error &&
                error.code === "NOT_FOUND"
                ? "legacy"
                : "failed",
            );
        },
      );
    return () => cancel.abort();
  }, [account, source, revision]);
  if (!account) return null;
  if (mode === "legacy") return legacy;
  if (mode === "checking") return <LoadingState label={t("loading")} />;
  if (mode === "failed")
    return (
      <div role="alert" className="flex flex-col items-start gap-2">
        <p>{t("loadFailed")}</p>
        <Button
          variant="outline"
          onClick={() => setRevision((value) => value + 1)}
        >
          {t("retry")}
        </Button>
      </div>
    );
  const completed = snapshot?.transfers
    .filter((item) => item.target === "drive" && item.phase === "complete")
    .map((item) => item.id)
    .join(",");
  return (
    <div className="flex min-w-0 flex-col gap-3">
      {progress}
      <DriveTreeBrowser
        renderCapacity={(data) => (
          <p className="text-sm text-muted-foreground">
            {formatSpace(data.usedBytes)} / {formatSpace(data.limitBytes)}
          </p>
        )}
        source={source.driveTree}
        refreshKey={completed}
        onLocationChange={onLocationChange}
        formatSize={formatSize}
        formatDate={formatStoredDate}
        filePreview={(file) => (
          <DrivePreviewAction source={source} file={file} disabled={false} />
        )}
        fileActions={(file) => [
          {
            id: "download",
            label: t("download"),
            onSelect: () => {
              void source
                .driveDownload(file.id)
                .catch((error) => toast.error(t(driveTreeError(error))));
            },
          },
        ]}
      />
    </div>
  );
}
