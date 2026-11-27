import type {
  MediaClient,
  MediaList,
  MediaQuery,
  MediaStatus,
} from "@assemble/media";
import { useCallback, useEffect, useRef, useState } from "react";
import { errorStatus, type Resource } from "./use-resource";

export function useShelf(
  client: MediaClient,
  query: MediaQuery,
  status: MediaStatus,
  page: number,
  sessionSignal: AbortSignal,
) {
  const [revision, setRevision] = useState(0);
  const mediaType = query.mediaType;
  const ownerId = query.ownerId;
  const context = JSON.stringify([
    client.scope,
    status,
    mediaType ?? null,
    ownerId ?? null,
  ]);
  const request = `${context}:${page}:${revision}`;
  const [result, setResult] = useState<{
    client: MediaClient;
    context: string;
    request: string;
    sessionSignal: AbortSignal;
    resource: Resource<MediaList>;
  } | null>(null);
  const latest = useRef(result);
  latest.current = result;
  useEffect(() => {
    const settled = latest.current;
    if (
      settled?.client === client &&
      settled.request === request &&
      settled.resource.phase === "ready"
    )
      return;
    const controller = new AbortController();
    const cancel = () => controller.abort();
    sessionSignal.addEventListener("abort", cancel, { once: true });
    if (sessionSignal.aborted) controller.abort();
    void client
      .list(
        {
          ownerId,
          mediaType,
          status,
          page,
          pageSize: 12,
          sort: status === "completed" ? "watched" : "updated",
        },
        controller.signal,
      )
      .then(
        (value) => {
          if (!controller.signal.aborted)
            setResult({
              client,
              context,
              request,
              sessionSignal,
              resource: { phase: "ready", value },
            });
        },
        (error: unknown) => {
          if (!controller.signal.aborted)
            setResult({
              client,
              context,
              request,
              sessionSignal,
              resource: { phase: "failed", status: errorStatus(error) },
            });
        },
      );
    return () => {
      controller.abort();
      sessionSignal.removeEventListener("abort", cancel);
    };
  }, [
    client,
    context,
    request,
    mediaType,
    ownerId,
    status,
    page,
    sessionSignal,
  ]);
  const current =
    result?.client === client &&
    result.request === request &&
    (result.resource.phase === "ready" ||
      result.sessionSignal === sessionSignal);
  // 只在同一账户、片库与类型中保留翻页前的列表；详情仍严格按请求隔离。
  // 个人主页的公开归属也必须隔离，不能复用另一个人的列表或迟到响应。
  const previous =
    result?.client === client &&
    result.context === context &&
    result.resource.phase === "ready";
  const resource: Resource<MediaList> =
    current || previous ? result.resource : { phase: "loading" };
  const retry = useCallback(() => setRevision((value) => value + 1), []);
  return { resource, pending: !current, retry };
}
