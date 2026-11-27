import {
  type MediaPageLocation,
  MediaWorkspace,
} from "@assemble/media/components/workspace";
import { useMediaTranslation } from "@assemble/media/i18n";
import { Button } from "@assemble/ui/button";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@assemble/ui/empty";
import { useMemo } from "react";
import type { MediaBridge } from "@/lib/bridge";
import { uiText, useDesktopTranslation } from "@/lib/i18n";

export function MediaPage({
  source,
  accountId,
  onSignIn,
  onBrowseLibrary,
  onLocationChange,
}: {
  source: MediaBridge;
  accountId: string | null;
  onSignIn: () => void;
  onBrowseLibrary?: () => void;
  onLocationChange?: (location: MediaPageLocation | null) => void;
}) {
  useDesktopTranslation();
  const { t } = useMediaTranslation();
  const client = useMemo(() => source.client("private"), [source]);
  return (
    <div className="flex flex-col gap-4">
      {accountId === null ? (
        <Empty className="min-h-64">
          <EmptyHeader>
            <EmptyTitle>{uiText("signInForPrivateLibrary")}</EmptyTitle>
            <EmptyDescription>
              {uiText("privateLibraryDescription")}
            </EmptyDescription>
          </EmptyHeader>
          <EmptyContent>
            <Button variant="outline" onClick={onSignIn}>
              {uiText("openAccountCenter")}
            </Button>
          </EmptyContent>
        </Empty>
      ) : (
        <MediaWorkspace
          key={accountId}
          client={client}
          toolbarActions={
            onBrowseLibrary ? (
              <Button variant="outline" onClick={onBrowseLibrary}>
                {t("publication.browseAll")}
              </Button>
            ) : undefined
          }
          imageLoader={source.image}
          canEdit
          accountKey={accountId ?? "anonymous"}
          onAccessFailure={onSignIn}
          onLocationChange={onLocationChange}
          onSignIn={onSignIn}
          onCopyMagnet={(entry) => source.copyMagnet(entry.scope, entry.id)}
        />
      )}
    </div>
  );
}
