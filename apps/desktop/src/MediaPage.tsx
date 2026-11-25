import type { MediaScope } from "@assemble/media";
import { MediaWorkspace } from "@assemble/media/components/workspace";
import { Button } from "@assemble/ui/button";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@assemble/ui/empty";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@assemble/ui/tabs";
import { useMemo, useState } from "react";
import type { MediaBridge } from "@/lib/bridge";
import { uiText, useDesktopTranslation } from "@/lib/i18n";

export function MediaPage({
  source,
  accountId,
  onSignIn,
}: {
  source: MediaBridge;
  accountId: string | null;
  onSignIn: () => void;
}) {
  useDesktopTranslation();
  const [scope, setScope] = useState<MediaScope>("shared");
  const client = useMemo(() => source.client(scope), [source, scope]);
  return (
    <Tabs
      value={scope}
      onValueChange={(value) => {
        if (value === "shared" || value === "private") setScope(value);
      }}
      className="flex flex-col gap-4"
    >
      <TabsList aria-label={uiText("mediaLibraryScope")}>
        <TabsTrigger value="shared">{uiText("sharedMediaLibrary")}</TabsTrigger>
        <TabsTrigger value="private">
          {uiText("privateMediaLibrary")}
        </TabsTrigger>
      </TabsList>
      <TabsContent value={scope}>
        {scope === "private" && accountId === null ? (
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
            key={`${scope}:${accountId ?? "anonymous"}`}
            client={client}
            imageLoader={source.image}
            canEdit={scope === "private" && accountId !== null}
            accountKey={accountId ?? "anonymous"}
            onAccessFailure={onSignIn}
            onSignIn={onSignIn}
            onCopyMagnet={(entry) => source.copyMagnet(entry.scope, entry.id)}
          />
        )}
      </TabsContent>
    </Tabs>
  );
}
