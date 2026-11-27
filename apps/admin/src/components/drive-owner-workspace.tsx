import { DriveTrashBrowser } from "@assemble/ui/drive-trash-browser";
import {
  DriveTreeBrowser,
  type DriveTreeLocation,
} from "@assemble/ui/drive-tree-browser";
import {
  isTrashDetail,
  isTrashListing,
  type TrashSource,
} from "@assemble/ui/lib/drive-trash";
import { useTrashTranslation } from "@assemble/ui/lib/drive-trash-i18n";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@assemble/ui/tabs";
import {
  type ComponentProps,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";
import {
  type AccessFailure,
  type AdminAccount,
  AdminRequestError,
  request,
} from "../lib/api";

type Props = ComponentProps<typeof DriveTreeBrowser> & {
  owner: AdminAccount;
  onAccessFailure: (failure: AccessFailure) => void;
};
export function DriveOwnerWorkspace({
  owner,
  onAccessFailure,
  onLocationChange,
  ...props
}: Props) {
  const { t } = useTrashTranslation();
  const [tab, setTab] = useState("files");
  const [opened, setOpened] = useState(false);
  const [fileLocation, setFileLocation] = useState<DriveTreeLocation | null>(
    null,
  );
  const [trashLocation, setTrashLocation] = useState<DriveTreeLocation | null>(
    null,
  );
  const showFiles = useCallback(() => setTab("files"), []);
  const source = useMemo<TrashSource>(() => {
    async function read<T>(
      path: string,
      guard: (value: unknown) => value is T,
      signal?: AbortSignal,
    ): Promise<T> {
      try {
        const value: unknown = await request(
          path,
          signal ?? new AbortController().signal,
        );
        if (!guard(value)) throw new AdminRequestError("unavailable");
        return value;
      } catch (error) {
        if (
          error instanceof AdminRequestError &&
          (error.kind === "signedOut" || error.kind === "forbidden")
        )
          onAccessFailure(error.kind);
        throw error;
      }
    }
    return {
      list: (q, page, pageSize, signal) =>
        read(
          `/v1/admin/drive/trash?${new URLSearchParams({ accountId: owner.id, q, page: String(page), pageSize: String(pageSize) })}`,
          isTrashListing,
          signal,
        ),
      detail: (id, query, signal) =>
        read(
          `/v1/admin/drive/trash/${encodeURIComponent(id)}?${new URLSearchParams({ accountId: owner.id, parentId: query.parentId, q: query.q, page: String(query.page), pageSize: String(query.pageSize) })}`,
          isTrashDetail,
          signal,
        ),
    };
  }, [owner.id, onAccessFailure]);
  useEffect(() => {
    onLocationChange?.(tab === "files" ? fileLocation : trashLocation);
  }, [tab, fileLocation, trashLocation, onLocationChange]);
  useEffect(() => () => onLocationChange?.(null), [onLocationChange]);
  return (
    <Tabs
      value={tab}
      onValueChange={(value) => {
        setTab(value);
        if (value === "trash") setOpened(true);
      }}
    >
      <TabsList>
        <TabsTrigger value="files">{t("files")}</TabsTrigger>
        <TabsTrigger value="trash">{t("trash")}</TabsTrigger>
      </TabsList>
      <TabsContent value="files" forceMount hidden={tab !== "files"}>
        <DriveTreeBrowser {...props} onLocationChange={setFileLocation} />
      </TabsContent>
      <TabsContent value="trash" forceMount hidden={tab !== "trash"}>
        {opened ? (
          <DriveTrashBrowser
            source={source}
            variant="management"
            defaultPageSize={20}
            formatSize={props.formatSize}
            formatDate={props.formatDate}
            onLocationChange={setTrashLocation}
            onFiles={showFiles}
          />
        ) : null}
      </TabsContent>
    </Tabs>
  );
}
