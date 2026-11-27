import {
  type SetStateAction,
  useCallback,
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
} from "react";

export interface UrlStateCodec<T> {
  read: (parameters: URLSearchParams) => T;
  write: (parameters: URLSearchParams, value: T) => void;
}

const LOCATION_CHANGE = "assemble:locationchange";
const snapshot = () => window.location.hash;
const emptySnapshot = () => "";
const noSubscription = () => () => {};
function subscribe(changed: () => void) {
  window.addEventListener("hashchange", changed);
  window.addEventListener("popstate", changed);
  window.addEventListener(LOCATION_CHANGE, changed);
  return () => {
    window.removeEventListener("hashchange", changed);
    window.removeEventListener("popstate", changed);
    window.removeEventListener(LOCATION_CHANGE, changed);
  };
}

function parameters(hash: string) {
  const separator = hash.indexOf("?");
  return new URLSearchParams(separator < 0 ? "" : hash.slice(separator + 1));
}

/** 只有显式指定路由的网页列表才写网址；桌面端等调用者仍使用组件内状态。 */
export function useUrlState<T>(codec: UrlStateCodec<T>, route?: string) {
  const hash = useSyncExternalStore(
    route ? subscribe : noSubscription,
    route ? snapshot : emptySnapshot,
    emptySnapshot,
  );
  const fromUrl = useMemo(() => codec.read(parameters(hash)), [codec, hash]);
  const [local, setLocal] = useState(() => codec.read(new URLSearchParams()));
  const set = useCallback(
    (change: SetStateAction<T>, replace = false) => {
      if (!route) {
        setLocal(change);
        return;
      }
      // 每次读取当前网址，避免同一操作中多个控件互相覆盖参数。
      const params = parameters(window.location.hash);
      const previous = codec.read(params);
      const next =
        typeof change === "function"
          ? (change as (previous: T) => T)(previous)
          : change;
      codec.write(params, next);
      const query = params.toString();
      const nextHash = `#${route}${query ? `?${query}` : ""}`;
      if (nextHash === window.location.hash) return;
      window.history[replace ? "replaceState" : "pushState"](
        null,
        "",
        nextHash,
      );
      // pushState 不发出 hashchange；同页内所有订阅者也要立即看到新参数。
      window.dispatchEvent(new Event(LOCATION_CHANGE));
    },
    [codec, route],
  );
  return [route ? fromUrl : local, set] as const;
}

export function urlInteger(
  value: string | null,
  fallback: number,
  max: number,
) {
  if (!value || !/^\d+$/.test(value)) return fallback;
  const number = Number(value);
  return Number.isSafeInteger(number) && number >= 1 && number <= max
    ? number
    : fallback;
}

export function setUrlParameter(
  parameters: URLSearchParams,
  key: string,
  value: string | number | undefined,
  fallback: string | number = "",
) {
  if (value === undefined || value === fallback) parameters.delete(key);
  else parameters.set(key, String(value));
}

export function usePageCorrection(
  page: { page: number; pageSize: number; total: number } | null,
  correct: (page: number, pageSize: number) => void,
) {
  const current = page?.page;
  const size = page?.pageSize;
  const total = page?.total;
  useEffect(() => {
    if (current === undefined || size === undefined || total === undefined)
      return;
    const last = Math.max(1, Math.ceil(total / size));
    if (current > last) correct(last, size);
  }, [current, size, total, correct]);
}
