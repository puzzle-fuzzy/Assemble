// 仅由用户打开扩展后注入；不改网页、不读取 Cookie，不抓取任意脚本文本。
import { type FoundSource, publicUrl, sourceKind } from "./protocol";
export function discover(
  root: Document,
  entries: PerformanceEntry[],
): { pageUrl: string; title: string; sources: FoundSource[] } {
  const pageUrl = root.location.href;
  const title = root.title.trim().slice(0, 160);
  const found = new Map<string, FoundSource>();
  function add(value: string, mime = "", label = title, video = false) {
    if (!value.trim()) return;
    const url = publicUrl(value, root.baseURI);
    if (!url) return;
    const kind = sourceKind(url.href, mime) || (video ? "video" : null);
    if (kind && found.size < 100 && !found.has(url.href))
      found.set(url.href, { url: url.href, kind, title: label.slice(0, 160) });
  }
  add(pageUrl);
  for (const media of Array.from(
    root.querySelectorAll<HTMLVideoElement>("video"),
  ).slice(0, 200)) {
    add(
      media.currentSrc,
      media.getAttribute("type") ?? "",
      media.getAttribute("aria-label") || title,
      true,
    );
    const src = media.getAttribute("src");
    if (src) add(src, "", title, true);
    for (const child of media.querySelectorAll<HTMLSourceElement>(
      "source[src]",
    ))
      add(child.src, child.type, title, true);
  }
  for (const link of Array.from(
    root.querySelectorAll<HTMLAnchorElement>("a[href]"),
  ).slice(0, 2000)) {
    if (found.size >= 100) break;
    if (/\.(?:m3u8|mp4|m4v|webm|mov)(?:[?#]|$)/i.test(link.href))
      add(link.href, "", link.getAttribute("download") || title);
  }
  for (const entry of entries.slice(-1000)) add(entry.name);
  return { pageUrl, title, sources: [...found.values()] };
}
// executeScript 的结果只交回扩展后台，不通过网页可伪造的 postMessage 入口。
if (
  typeof document !== "undefined" &&
  typeof chrome !== "undefined" &&
  chrome.runtime?.id
) {
  (
    globalThis as typeof globalThis & {
      __assembleDetected?: ReturnType<typeof discover>;
    }
  ).__assembleDetected = discover(
    document,
    performance.getEntriesByType("resource"),
  );
}
