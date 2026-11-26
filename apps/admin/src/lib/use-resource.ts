import { useCallback, useEffect, useState } from "react";
import {
  type AccessFailure,
  AdminRequestError,
  type RequestFailure,
  request,
} from "./api";

export type Resource<T> =
  | { phase: "loading" }
  | { phase: "ready"; value: T }
  | { phase: "error"; failure: RequestFailure };

export function useResource<T>(
  path: string,
  onAccessFailure?: (failure: AccessFailure) => void,
) {
  const [revision, setRevision] = useState(0);
  const [result, setResult] = useState<{
    path: string;
    revision: number;
    resource: Resource<T>;
  }>({ path, revision, resource: { phase: "loading" } });
  const retry = useCallback(() => setRevision((value) => value + 1), []);

  useEffect(() => {
    const controller = new AbortController();
    setResult({ path, revision, resource: { phase: "loading" } });
    request<T>(path, controller.signal)
      .then((value) => {
        if (!controller.signal.aborted) {
          setResult({ path, revision, resource: { phase: "ready", value } });
        }
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        const failure =
          error instanceof AdminRequestError ? error.kind : "unavailable";
        if (failure === "signedOut" || failure === "forbidden") {
          onAccessFailure?.(failure);
        }
        setResult({ path, revision, resource: { phase: "error", failure } });
      });
    return () => controller.abort();
  }, [path, revision, onAccessFailure]);

  // 新查询一经提交就隐藏旧数据，即使其 effect 尚未执行也不会短暂回显旧页。
  const resource: Resource<T> =
    result.path === path && result.revision === revision
      ? result.resource
      : { phase: "loading" };
  const update = useCallback(
    (change: (value: T) => T) => {
      setResult((previous) =>
        previous.path === path &&
        previous.revision === revision &&
        previous.resource.phase === "ready"
          ? {
              ...previous,
              resource: {
                phase: "ready",
                value: change(previous.resource.value),
              },
            }
          : previous,
      );
    },
    [path, revision],
  );
  return { resource, retry, update };
}
