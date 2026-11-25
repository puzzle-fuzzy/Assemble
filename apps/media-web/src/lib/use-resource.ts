import { useCallback, useEffect, useRef, useState } from "react";

export type Resource<T> =
  | { phase: "loading" }
  | { phase: "ready"; value: T }
  | { phase: "failed"; status: number | null };

export function errorStatus(error: unknown): number | null {
  return error !== null &&
    typeof error === "object" &&
    "status" in error &&
    typeof error.status === "number"
    ? error.status
    : null;
}

export function useResource<T>(
  key: string,
  load: (signal: AbortSignal) => Promise<T>,
) {
  const loader = useRef(load);
  loader.current = load;
  const [revision, setRevision] = useState(0);
  const requestKey = `${key}:${revision}`;
  const [result, setResult] = useState<{
    key: string;
    resource: Resource<T>;
  } | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    void loader.current(controller.signal).then(
      (value) => {
        if (!controller.signal.aborted)
          setResult({ key: requestKey, resource: { phase: "ready", value } });
      },
      (error: unknown) => {
        if (!controller.signal.aborted)
          setResult({
            key: requestKey,
            resource: { phase: "failed", status: errorStatus(error) },
          });
      },
    );
    return () => controller.abort();
  }, [requestKey]);
  const retry = useCallback(() => setRevision((value) => value + 1), []);
  // 数据必须属于当前请求；路由改变后的第一帧也不能显示上一条私人资料。
  const resource: Resource<T> =
    result?.key === requestKey ? result.resource : { phase: "loading" };
  return { resource, retry };
}
