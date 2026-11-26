import { useEffect, useRef, useState } from "react";
import { type AccessFailure, AdminRequestError, request } from "./api";
import { useResource } from "./use-resource";

/** 后台刷新保留当前行和焦点；改变查询仍由 useResource 立即隐藏旧结果。 */
export function useDhtResource<T>(
  path: string,
  onAccessFailure: (failure: AccessFailure) => void,
  interval: number | null,
  pollWhen?: (value: T) => boolean,
) {
  const result = useResource<T>(path, onAccessFailure);
  const [refreshFailed, setRefreshFailed] = useState(false);
  const ready = useRef(false);
  ready.current =
    result.resource.phase === "ready" &&
    (pollWhen?.(result.resource.value) ?? true);
  const { update } = result;
  useEffect(() => {
    setRefreshFailed(false);
    if (interval === null) return;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async () => {
      if (document.visibilityState !== "hidden" && ready.current) {
        try {
          const value = await request<T>(path, controller.signal);
          if (!controller.signal.aborted) {
            update(() => value);
            setRefreshFailed(false);
          }
        } catch (error) {
          if (controller.signal.aborted) return;
          if (
            error instanceof AdminRequestError &&
            (error.kind === "signedOut" || error.kind === "forbidden")
          ) {
            onAccessFailure(error.kind);
            return;
          }
          setRefreshFailed(true);
        }
      }
      if (!controller.signal.aborted) timer = setTimeout(refresh, interval);
    };
    timer = setTimeout(refresh, interval);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [path, interval, onAccessFailure, update]);
  return { ...result, refreshFailed };
}
