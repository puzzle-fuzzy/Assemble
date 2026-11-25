import { useCallback, useEffect, useRef, useState } from "react";

export type MediaAccessFailure = "signedOut" | "forbidden";
export function mediaErrorCode(error: unknown): string {
  if (error !== null && typeof error === "object") {
    if ("code" in error && error.code === "CSRF_REJECTED")
      return "CSRF_REJECTED";
    if ("status" in error && error.status === 401) return "UNAUTHORIZED";
    if ("status" in error && error.status === 403) return "FORBIDDEN";
    if ("code" in error && typeof error.code === "string") return error.code;
  }
  return "generic";
}

export function useMediaTask(
  onAccessFailure?: (failure: MediaAccessFailure) => void,
) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const active = useRef<AbortController | null>(null);
  const failureRef = useRef(onAccessFailure);
  failureRef.current = onAccessFailure;
  const cancel = useCallback(() => {
    active.current?.abort();
    active.current = null;
    setPending(false);
  }, []);
  useEffect(() => () => active.current?.abort(), []);
  const run = useCallback(
    async <T>(
      operation: (signal: AbortSignal) => Promise<T>,
      done: (value: T) => void,
    ) => {
      active.current?.abort();
      const controller = new AbortController();
      active.current = controller;
      setPending(true);
      setError(null);
      try {
        const value = await operation(controller.signal);
        if (!controller.signal.aborted) done(value);
      } catch (caught) {
        if (!controller.signal.aborted) {
          setError(caught);
          const code = mediaErrorCode(caught);
          if (code === "UNAUTHORIZED") failureRef.current?.("signedOut");
          if (code === "FORBIDDEN") failureRef.current?.("forbidden");
        }
      } finally {
        if (!controller.signal.aborted) {
          active.current = null;
          setPending(false);
        }
      }
    },
    [],
  );
  return { run, pending, error, cancel, clearError: () => setError(null) };
}
