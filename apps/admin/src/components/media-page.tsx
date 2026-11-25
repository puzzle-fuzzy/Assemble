import {
  browserImageLoader,
  createBrowserMediaClient,
  createBrowserMediaCollectionsClient,
  getMediaStatistics,
} from "@assemble/media/browser-client";
import { MediaCollectionsManager } from "@assemble/media/components/collections";
import { MediaStatisticsView } from "@assemble/media/components/statistics";
import { MediaWorkspace } from "@assemble/media/components/workspace";
import { useMediaTranslation } from "@assemble/media/i18n";
import type { MediaEntry } from "@assemble/media/model";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@assemble/ui/tabs";
import { useEffect, useMemo, useRef } from "react";
import type { AccessFailure, AdminSession } from "../lib/api";

export function MediaPage({
  session,
  onAccessFailure,
}: {
  session: AdminSession;
  onAccessFailure: (failure: AccessFailure) => void;
}) {
  const { t } = useMediaTranslation();
  const copyScope = useRef<{
    owner: string;
    csrf: string;
    controller: AbortController;
  } | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    copyScope.current = {
      owner: session.account.id,
      csrf: session.csrfToken,
      controller,
    };
    return () => controller.abort();
  }, [session.account.id, session.csrfToken]);
  const { library, collections } = useMemo(() => {
    const options = { csrfToken: () => session.csrfToken };
    return {
      library: createBrowserMediaClient("shared", options),
      collections: createBrowserMediaCollectionsClient(options),
    };
  }, [session.csrfToken]);
  async function copyMagnet(entry: MediaEntry) {
    // 复制前重新请求，网页会话失效后不使用先前详情里保留的磁力。
    const scope = copyScope.current;
    if (
      !scope ||
      scope.owner !== session.account.id ||
      scope.csrf !== session.csrfToken
    )
      throw new Error("MEDIA_SESSION_CHANGED");
    const signal = scope.controller.signal;
    signal.throwIfAborted();
    const current = await library.get(entry.id, signal);
    signal.throwIfAborted();
    if (!current.magnetUri) throw new Error("MEDIA_MAGNET_UNAVAILABLE");
    await navigator.clipboard.writeText(current.magnetUri);
  }
  return (
    <Tabs defaultValue="library">
      <TabsList>
        <TabsTrigger value="library">{t("scope.shared")}</TabsTrigger>
        <TabsTrigger value="collections">{t("collections.title")}</TabsTrigger>
        <TabsTrigger value="statistics">{t("statistics.title")}</TabsTrigger>
      </TabsList>
      <TabsContent value="library">
        <MediaWorkspace
          client={library}
          imageLoader={browserImageLoader}
          canEdit
          accountKey={session.account.id}
          onAccessFailure={onAccessFailure}
          onCopyMagnet={copyMagnet}
        />
      </TabsContent>
      <TabsContent value="collections">
        <MediaCollectionsManager
          client={collections}
          libraryClient={library}
          imageLoader={browserImageLoader}
          accountKey={session.account.id}
          onAccessFailure={onAccessFailure}
          onCopyMagnet={copyMagnet}
        />
      </TabsContent>
      <TabsContent value="statistics">
        <MediaStatisticsView
          load={getMediaStatistics}
          accountKey={session.account.id}
          onAccessFailure={onAccessFailure}
        />
      </TabsContent>
    </Tabs>
  );
}
