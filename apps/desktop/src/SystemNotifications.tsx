import type {
  SystemNotification,
  SystemNotificationRead,
  SystemNotifications,
} from "@assemble/ui/lib/system-notifications";
import { SystemNotificationCenter } from "@assemble/ui/system-notifications";
import { useCallback, useEffect, useRef, useState } from "react";
import type { Bridge } from "@/lib/bridge";

export function DesktopSystemNotifications({
  source,
  view,
  onNavigate,
  canNavigate,
}: {
  source: Bridge;
  view?: SystemNotifications;
  onNavigate: (item: SystemNotification) => void;
  canNavigate?: (item: SystemNotification) => boolean;
}) {
  const [readError, setReadError] = useState(false);
  const [error, setError] = useState(false);
  const [pending, setPending] = useState(false);
  const mounted = useRef(true);
  const active = useRef(false);
  const syncing = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const refresh = useCallback(async () => {
    if (active.current || syncing.current) return;
    syncing.current = true;
    try {
      await source.systemNotificationsRefresh();
      if (mounted.current) setError(false);
    } catch {
      if (mounted.current) setError(true);
    } finally {
      syncing.current = false;
    }
  }, [source]);
  useEffect(() => {
    const visible = () => {
      if (document.visibilityState !== "hidden") void refresh();
    };
    window.addEventListener("focus", visible);
    document.addEventListener("visibilitychange", visible);
    return () => {
      window.removeEventListener("focus", visible);
      document.removeEventListener("visibilitychange", visible);
    };
  }, [refresh]);
  useEffect(() => {
    if (!error && view?.available !== false) return;
    const timer = window.setInterval(() => {
      if (document.visibilityState !== "hidden") void refresh();
    }, 30_000);
    return () => window.clearInterval(timer);
  }, [error, view?.available, refresh]);
  async function read(items: SystemNotificationRead[]) {
    if (active.current) return;
    active.current = true;
    setPending(true);
    setReadError(false);
    try {
      await source.systemNotificationsRead(items);
    } catch {
      if (mounted.current) setReadError(true);
    } finally {
      active.current = false;
      if (mounted.current) setPending(false);
    }
  }
  return (
    <SystemNotificationCenter
      items={view?.items ?? []}
      error={error || view?.available === false}
      pending={pending}
      readError={readError}
      onRead={read}
      onNavigate={onNavigate}
      canNavigate={canNavigate}
    />
  );
}
