import {
  isSystemNotifications,
  type SystemNotificationRead,
  type SystemNotifications,
} from "@assemble/ui/lib/system-notifications";
import { SystemNotificationCenter } from "@assemble/ui/system-notifications";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  type AccessFailure,
  AdminRequestError,
  type AdminSession,
  request,
} from "../lib/api";

export function AdminSystemNotifications({
  session,
  onAccessFailure,
  onStorage,
}: {
  session: AdminSession;
  onAccessFailure: (failure: AccessFailure) => void;
  onStorage: () => void;
}) {
  const [view, setView] = useState<SystemNotifications>({
    items: [],
    available: true,
  });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);
  const [readError, setReadError] = useState(false);
  const [pending, setPending] = useState(false);
  const get = useRef<AbortController | null>(null);
  const write = useRef<AbortController | null>(null);
  const access = useCallback(
    (error: unknown) => {
      if (
        error instanceof AdminRequestError &&
        (error.kind === "signedOut" || error.kind === "forbidden")
      ) {
        setView({ items: [], available: false });
        onAccessFailure(error.kind);
      }
    },
    [onAccessFailure],
  );
  const refresh = useCallback(async () => {
    if (write.current) return;
    get.current?.abort();
    const controller = new AbortController();
    get.current = controller;
    setLoading(true);
    try {
      const result = await request<unknown>(
        "/v1/admin/notifications",
        controller.signal,
      );
      if (!isSystemNotifications(result))
        throw new Error("INVALID_SYSTEM_NOTIFICATIONS");
      if (!controller.signal.aborted) {
        setView(result);
        setError(false);
      }
    } catch (failure: unknown) {
      if (!controller.signal.aborted) {
        access(failure);
        setError(true);
      }
    } finally {
      if (!controller.signal.aborted) setLoading(false);
    }
  }, [access]);
  useEffect(() => {
    void refresh();
    const visible = () => {
      if (document.visibilityState !== "hidden") void refresh();
    };
    const timer = window.setInterval(visible, 30_000);
    document.addEventListener("visibilitychange", visible);
    window.addEventListener("focus", visible);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", visible);
      window.removeEventListener("focus", visible);
      get.current?.abort();
      write.current?.abort();
    };
  }, [refresh]);
  async function read(items: SystemNotificationRead[]) {
    if (write.current) return;
    get.current?.abort();
    setLoading(false);
    const controller = new AbortController();
    write.current = controller;
    setPending(true);
    setReadError(false);
    try {
      const result = await request<unknown>(
        "/v1/admin/notifications/read",
        controller.signal,
        { csrfToken: session.csrfToken, body: { items } },
      );
      if (!isSystemNotifications(result))
        throw new Error("INVALID_SYSTEM_NOTIFICATIONS");
      if (!controller.signal.aborted) {
        setView(result);
        setError(false);
      }
    } catch (failure: unknown) {
      if (!controller.signal.aborted) {
        access(failure);
        setReadError(true);
      }
    } finally {
      if (write.current === controller) write.current = null;
      if (!controller.signal.aborted) setPending(false);
    }
  }
  return (
    <SystemNotificationCenter
      items={view.items}
      loading={loading}
      error={error || !view.available}
      pending={pending}
      readError={readError}
      onRead={read}
      timeZone="Asia/Shanghai"
      onNavigate={onStorage}
    />
  );
}
