// 拖拽只请求新的数据顺序，行节点仍由 React 的稳定 key 管理，不手动改写 tbody。
import {
  type KeyboardEvent,
  type PointerEvent,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

interface Options {
  ids: string[];
  dataIdentity: unknown;
  onMove: (id: string, targetId: string) => void;
  announce: (id: string, position: number) => string;
  cancelled: string;
}
interface Drag {
  id: string;
  pointer: number;
  handle: HTMLButtonElement;
  row: HTMLTableRowElement;
  body: HTMLTableSectionElement;
  scroller: HTMLElement;
  startY: number;
  y: number;
  scrollTop: number;
  moved: boolean;
  target: string;
  frame: number;
  lastTime: number;
}
type Announcement = { id: string; position: number } | { cancelled: true };
let pointerActive = false;
const rowsOf = (body: HTMLElement) => [
  ...body.querySelectorAll<HTMLTableRowElement>("tr[data-table-row]"),
];
function scrollerOf(row: HTMLElement): HTMLElement {
  for (let node = row.parentElement; node; node = node.parentElement) {
    if (
      node.scrollHeight > node.clientHeight &&
      /auto|scroll/.test(getComputedStyle(node).overflowY)
    )
      return node;
  }
  return (document.scrollingElement ?? document.documentElement) as HTMLElement;
}
function clearLift(row: HTMLElement) {
  row.style.removeProperty("translate");
  row.removeAttribute("data-table-dragging");
}

export function useTableReorder(options: Options) {
  const latest = useRef(options);
  latest.current = options;
  const drag = useRef<Drag | null>(null);
  const escapeListener = useRef<
    ((event: globalThis.KeyboardEvent) => void) | null
  >(null);
  const [drop, setDrop] = useState<{
    id: string;
    edge: "before" | "after";
  } | null>(null);
  const [announcement, setAnnouncement] = useState<Announcement | null>(null);
  const committed = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pending = useRef<{
    id: string;
    orderKey: string;
    rows: { row: HTMLTableRowElement; rect: DOMRect }[];
  } | null>(null);
  const orderKey = JSON.stringify(options.ids);
  const report = useCallback((value: Announcement) => {
    if (timer.current) clearTimeout(timer.current);
    setAnnouncement(null);
    timer.current = setTimeout(() => setAnnouncement(value), 500);
  }, []);
  const release = useCallback(() => {
    const current = drag.current;
    if (!current) return;
    drag.current = null;
    cancelAnimationFrame(current.frame);
    clearLift(current.row);
    pointerActive = false;
    document.documentElement.classList.remove("table-row-dragging");
    if (escapeListener.current)
      window.removeEventListener("keydown", escapeListener.current, true);
    escapeListener.current = null;
    if (current.handle.hasPointerCapture?.(current.pointer))
      current.handle.releasePointerCapture(current.pointer);
    setDrop(null);
  }, []);
  function cancelOnEscape(event: globalThis.KeyboardEvent) {
    if (event.key !== "Escape" || !drag.current) return;
    event.preventDefault();
    event.stopPropagation();
    const row = drag.current.row;
    const handle = drag.current.handle;
    release();
    row.scrollIntoView({ block: "nearest", inline: "nearest" });
    handle.focus({ preventScroll: true });
    report({ cancelled: true });
  }
  useEffect(() => {
    // 查询、分页或账户换掉了数据，旧拖拽不能作用于新结果集。
    void orderKey;
    void options.dataIdentity;
    release();
    if (!committed.current) {
      if (timer.current) clearTimeout(timer.current);
      setAnnouncement(null);
    }
    committed.current = false;
  }, [orderKey, options.dataIdentity, release]);
  useEffect(() => {
    return () => {
      release();
      if (timer.current) clearTimeout(timer.current);
    };
  }, [release]);
  useLayoutEffect(() => {
    void orderKey;
    // 同标识的新查询结果也要丢弃上一份数据的未提交移动。
    void options.dataIdentity;
    const move = pending.current;
    if (!move) return;
    pending.current = null;
    if (move.orderKey !== orderKey) return;
    committed.current = true;
    const moved = move.rows.find(
      (item) => item.row.dataset.tableRow === move.id,
    )?.row;
    moved?.scrollIntoView({ block: "nearest", inline: "nearest" });
    moved
      ?.querySelector<HTMLButtonElement>("[data-table-drag-handle]")
      ?.focus({ preventScroll: true });
    if (!window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      for (const item of move.rows) {
        const next = item.row.getBoundingClientRect();
        const delta = item.rect.top - next.top;
        if (delta && item.row.animate)
          item.row.animate(
            [{ translate: `0 ${delta}px` }, { translate: "0 0" }],
            { duration: 150, easing: "ease-out" },
          );
      }
    }
    const position = latest.current.ids.indexOf(move.id);
    if (position >= 0) report({ id: move.id, position: position + 1 });
  }, [orderKey, options.dataIdentity, report]);
  function commit(id: string, target: string, body: HTMLTableSectionElement) {
    if (id === target || !latest.current.ids.includes(target)) return;
    const ids = [...latest.current.ids];
    const from = ids.indexOf(id);
    const to = ids.indexOf(target);
    if (from < 0 || to < 0) return;
    ids.splice(from, 1);
    ids.splice(to, 0, id);
    const rows = rowsOf(body);
    for (const row of rows) {
      if (typeof row.getAnimations === "function")
        for (const animation of row.getAnimations()) animation.finish();
    }
    pending.current = {
      id,
      orderKey: JSON.stringify(ids),
      rows: rows.map((row) => ({
        row,
        rect: row.getBoundingClientRect(),
      })),
    };
    latest.current.onMove(id, target);
  }
  function update(current: Drag) {
    const bounds = current.body.getBoundingClientRect();
    const y = Math.min(bounds.bottom, Math.max(bounds.top, current.y));
    const siblings = rowsOf(current.body).filter((row) => row !== current.row);
    const before = siblings.findIndex((row) => {
      const rect = row.getBoundingClientRect();
      return y < rect.top + rect.height / 2;
    });
    const index = before < 0 ? siblings.length : before;
    current.target = latest.current.ids[index] ?? current.id;
    const oldIndex = latest.current.ids.indexOf(current.id);
    const next =
      current.target === current.id
        ? null
        : {
            id: current.target,
            edge: index < oldIndex ? ("before" as const) : ("after" as const),
          };
    setDrop((previous) =>
      previous?.id === next?.id && previous?.edge === next?.edge
        ? previous
        : next,
    );
    current.row.style.translate = `0 ${current.y - current.startY + current.scroller.scrollTop - current.scrollTop}px`;
  }
  function scrollFrame(time: number) {
    const current = drag.current;
    if (!current) return;
    if (current.moved) {
      const rect =
        current.scroller === document.scrollingElement
          ? { top: 0, bottom: window.innerHeight, height: window.innerHeight }
          : current.scroller.getBoundingClientRect();
      const edge = Math.min(current.row.offsetHeight || 48, rect.height / 3);
      const distance =
        current.y < rect.top + edge
          ? current.y - rect.top - edge
          : current.y > rect.bottom - edge
            ? current.y - rect.bottom + edge
            : 0;
      if (edge && distance)
        current.scroller.scrollTop +=
          (Math.max(-1, Math.min(1, distance / edge)) *
            700 *
            Math.min(32, time - current.lastTime)) /
          1000;
      update(current);
    }
    current.lastTime = time;
    current.frame = requestAnimationFrame(scrollFrame);
  }
  function pointerDown(event: PointerEvent<HTMLButtonElement>, id: string) {
    if (event.button !== 0 || !event.isPrimary || pointerActive) return;
    const handle = event.currentTarget;
    const row = handle.closest<HTMLTableRowElement>("tr");
    const body = row?.parentElement;
    if (!row || !(body instanceof HTMLTableSectionElement)) return;
    event.preventDefault();
    handle.focus({ preventScroll: true });
    handle.setPointerCapture(event.pointerId);
    const scroller = scrollerOf(row);
    drag.current = {
      id,
      pointer: event.pointerId,
      handle,
      row,
      body,
      scroller,
      startY: event.clientY,
      y: event.clientY,
      scrollTop: scroller.scrollTop,
      moved: false,
      target: id,
      frame: 0,
      lastTime: performance.now(),
    };
    pointerActive = true;
    document.documentElement.classList.add("table-row-dragging");
    window.addEventListener("keydown", cancelOnEscape, true);
    escapeListener.current = cancelOnEscape;
    drag.current.frame = requestAnimationFrame(scrollFrame);
  }
  function pointerMove(event: PointerEvent<HTMLButtonElement>) {
    const current = drag.current;
    if (!current || current.pointer !== event.pointerId) return;
    current.y = event.clientY;
    if (Math.abs(current.y - current.startY) < 4 && !current.moved) return;
    current.moved = true;
    current.row.setAttribute("data-table-dragging", "true");
    update(current);
  }
  function pointerUp(event: PointerEvent<HTMLButtonElement>) {
    const current = drag.current;
    if (!current || current.pointer !== event.pointerId) return;
    if (current.moved) {
      current.y = event.clientY;
      update(current);
      commit(current.id, current.target, current.body);
    }
    release();
  }
  function keyboard(event: KeyboardEvent<HTMLButtonElement>, id: string) {
    if (!event.altKey || !["ArrowUp", "ArrowDown"].includes(event.key)) return;
    event.preventDefault();
    const index = latest.current.ids.indexOf(id);
    const target =
      latest.current.ids[index + (event.key === "ArrowUp" ? -1 : 1)];
    const body = event.currentTarget.closest("tbody");
    if (target && body) commit(id, target, body);
  }
  return {
    drop,
    message: announcement
      ? "cancelled" in announcement
        ? options.cancelled
        : options.announce(announcement.id, announcement.position)
      : "",
    handle: (id: string) => ({
      onPointerDown: (event: PointerEvent<HTMLButtonElement>) =>
        pointerDown(event, id),
      onPointerMove: pointerMove,
      onPointerUp: pointerUp,
      onPointerCancel: (event: PointerEvent<HTMLButtonElement>) => {
        if (drag.current?.pointer === event.pointerId) release();
      },
      onLostPointerCapture: (event: PointerEvent<HTMLButtonElement>) => {
        if (drag.current?.pointer === event.pointerId) release();
      },
      onKeyDown: (event: KeyboardEvent<HTMLButtonElement>) =>
        keyboard(event, id),
    }),
  };
}
