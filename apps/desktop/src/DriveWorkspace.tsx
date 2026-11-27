import { Button } from "@assemble/ui/button";
import {
  DriveTrashBrowser,
  TrashConfirm,
} from "@assemble/ui/drive-trash-browser";
import {
  DriveTreeBrowser,
  type DriveTreeLocation,
} from "@assemble/ui/drive-tree-browser";
import { useTrashTranslation } from "@assemble/ui/lib/drive-trash-i18n";
import type { DriveEntry } from "@assemble/ui/lib/drive-tree";
import {
  driveTreeError,
  useDriveTreeTranslation,
} from "@assemble/ui/lib/drive-tree-i18n";
import { LoadingState } from "@assemble/ui/loading-state";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@assemble/ui/tabs";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
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
  const { t: trashText } = useTrashTranslation();
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
  if (mode === "legacy")
    return (
      <>
        {legacy}
        <p role="status" className="text-sm text-muted-foreground">
          {trashText("unavailable")}
        </p>
      </>
    );
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
  return (
    <DriveFilesAndTrash
      source={source}
      snapshot={snapshot}
      onLocationChange={onLocationChange}
      progress={progress}
    />
  );
}

function DriveFilesAndTrash({
  source,
  snapshot,
  onLocationChange,
  progress,
}: {
  source: Bridge;
  snapshot: Snapshot | null;
  onLocationChange: (location: DriveTreeLocation | null) => void;
  progress?: ReactNode;
}) {
  const { t } = useDriveTreeTranslation();
  const { t: trashText } = useTrashTranslation();
  const [tab, setTab] = useState("files");
  const [capability, setCapability] = useState<
    "checking" | "ready" | "unavailable" | "failed"
  >("checking");
  const [probe, setProbe] = useState(0);
  const [trashRevision, setTrashRevision] = useState(0);
  const [fileLocation, setFileLocation] = useState<DriveTreeLocation | null>(
    null,
  );
  const [trashLocation, setTrashLocation] = useState<DriveTreeLocation | null>(
    null,
  );
  const [initialParent, setInitialParent] = useState({
    id: "root",
    generation: 0,
  });
  const [recycling, setRecycling] = useState<{
    entry: DriveEntry;
    done: () => void;
  } | null>(null);
  const trashOperation = useMemo(
    () =>
      recycling ? { mode: "move" as const, entry: recycling.entry } : null,
    [recycling],
  );
  const filesPane = useRef<HTMLDivElement>(null);
  const filesSource = useMemo(
    () => ({ ...source.driveTree, remove: undefined }),
    [source],
  );
  const showFiles = useCallback(() => setTab("files"), []);
  useEffect(() => {
    void probe;
    const cancel = new AbortController();
    setCapability("checking");
    void source.driveTrash.list("", 1, 10, cancel.signal).then(
      () => {
        if (!cancel.signal.aborted) setCapability("ready");
      },
      (error) => {
        if (!cancel.signal.aborted)
          setCapability(
            typeof error === "object" &&
              error !== null &&
              "code" in error &&
              error.code === "NOT_FOUND"
              ? "unavailable"
              : "failed",
          );
      },
    );
    return () => cancel.abort();
  }, [source, probe]);
  useEffect(() => {
    onLocationChange(tab === "files" ? fileLocation : trashLocation);
  }, [tab, fileLocation, trashLocation, onLocationChange]);
  function close() {
    const id = recycling?.entry.id;
    setRecycling(null);
    requestAnimationFrame(() => {
      const target = id
        ? filesPane.current?.querySelector<HTMLElement>(
            `[data-drive-entry-id="${id}"]`,
          )
        : null;
      (target ?? filesPane.current)?.focus();
    });
  }
  const completed = snapshot?.transfers
    .filter((item) => item.target === "drive" && item.phase === "complete")
    .map((item) => item.id)
    .join(",");
  return (
    <div className="flex min-w-0 flex-col gap-3">
      {progress}
      <Tabs value={tab} onValueChange={setTab}>
        <TabsList>
          <TabsTrigger value="files">{trashText("files")}</TabsTrigger>
          <TabsTrigger value="trash" disabled={capability !== "ready"}>
            {trashText("trash")}
          </TabsTrigger>
        </TabsList>
        <TabsContent value="files" forceMount hidden={tab !== "files"}>
          <div ref={filesPane} tabIndex={-1} className="outline-none">
            <DriveTreeBrowser
              key={initialParent.generation}
              initialParentId={initialParent.id}
              renderCapacity={(data) => (
                <p className="text-sm text-muted-foreground">
                  {formatSpace(data.usedBytes)} / {formatSpace(data.limitBytes)}
                </p>
              )}
              source={filesSource}
              onTrash={
                capability === "ready"
                  ? (entry, done) => setRecycling({ entry, done })
                  : undefined
              }
              removeLabel={trashText("move")}
              refreshKey={completed}
              onLocationChange={setFileLocation}
              formatSize={formatSize}
              formatDate={formatStoredDate}
              filePreview={(file) => (
                <DrivePreviewAction
                  source={source}
                  file={file}
                  disabled={false}
                />
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
            {capability === "unavailable" ? (
              <p role="status" className="text-sm text-muted-foreground">
                {trashText("unavailable")}
              </p>
            ) : capability === "failed" ? (
              <div role="alert" className="flex items-center gap-2">
                <p>{trashText("failed")}</p>
                <Button
                  variant="outline"
                  onClick={() => setProbe((v) => v + 1)}
                >
                  {trashText("retry")}
                </Button>
              </div>
            ) : null}
          </div>
        </TabsContent>
        <TabsContent value="trash" forceMount hidden={tab !== "trash"}>
          {capability === "ready" ? (
            <DriveTrashBrowser
              source={source.driveTrash}
              formatSize={formatSize}
              formatDate={formatStoredDate}
              onLocationChange={setTrashLocation}
              onFiles={showFiles}
              refreshKey={trashRevision}
              onRestored={(restored) => {
                setInitialParent((previous) => ({
                  id: restored.parentId ?? "root",
                  generation: previous.generation + 1,
                }));
                setTab("files");
              }}
            />
          ) : null}
        </TabsContent>
      </Tabs>
      {recycling && trashOperation ? (
        <TrashConfirm
          source={source.driveTrash}
          operation={trashOperation}
          formatSize={formatSize}
          onClose={close}
          onSaved={() => {
            const done = recycling.done;
            close();
            done();
            setTrashRevision((v) => v + 1);
            toast.success(trashText("moved"));
          }}
        />
      ) : null}
    </div>
  );
}
