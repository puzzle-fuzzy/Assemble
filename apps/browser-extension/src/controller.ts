import {
  type FoundSource,
  isNativeReply,
  NATIVE_HOST,
  type PopupReply,
  type PopupRequest,
  publicUrl,
  sourceKind,
  sourceView,
  type TabSources,
} from "./protocol";
export const tabKey = (id: number) => `sources:${id}`;
const TTL = 10 * 60 * 1000;
export interface ExtensionApi {
  storage: Pick<typeof chrome.storage, "session">;
  tabs: Pick<typeof chrome.tabs, "query" | "get">;
  scripting: Pick<typeof chrome.scripting, "executeScript">;
  runtime: Pick<typeof chrome.runtime, "sendNativeMessage">;
  permissions: Pick<typeof chrome.permissions, "contains">;
}
export class Controller {
  constructor(private api: ExtensionApi) {}
  async current() {
    const [tab] = await this.api.tabs.query({
      active: true,
      currentWindow: true,
    });
    if (typeof tab?.id !== "number" || !tab.url || !publicUrl(tab.url))
      throw new Error("PAGE_UNAVAILABLE");
    return { id: tab.id, url: tab.url };
  }
  async read(id: number): Promise<TabSources | null> {
    const key = tabKey(id);
    const value = (await this.api.storage.session.get(key))[key] as
      | TabSources
      | undefined;
    return value && Date.now() - value.updatedAt < TTL ? value : null;
  }
  async scan(id: number, url: string) {
    await this.api.scripting.executeScript({
      target: { tabId: id },
      files: ["detector.js"],
    });
    const results = await this.api.scripting.executeScript({
      target: { tabId: id },
      func: () => {
        const root = globalThis as typeof globalThis & {
          __assembleDetected?: {
            pageUrl: string;
            title: string;
            sources: FoundSource[];
          };
        };
        const result = root.__assembleDetected;
        delete root.__assembleDetected;
        return result;
      },
    });
    const result = results[0]?.result;
    if (!result || result.pageUrl !== url || !Array.isArray(result.sources))
      throw new Error("PAGE_UNAVAILABLE");
    const previous = await this.read(id);
    const all = Object.entries(await this.api.storage.session.get(null))
      .filter(([key]) => key.startsWith("sources:"))
      .sort(
        (a, b) =>
          (b[1] as TabSources).updatedAt - (a[1] as TabSources).updatedAt,
      );
    const expired = all
      .filter(
        ([key, value], index) =>
          key !== tabKey(id) &&
          (index >= 23 || Date.now() - (value as TabSources).updatedAt >= TTL),
      )
      .map(([key]) => key);
    if (expired.length) await this.api.storage.session.remove(expired);
    const capturing = previous?.pageUrl === url ? previous.capturing : false;
    const state: TabSources = {
      pageUrl: url,
      title: result.title.slice(0, 160),
      updatedAt: Date.now(),
      capturing,
      sources: previous?.pageUrl === url ? previous.sources : [],
    };
    merge(state, result.sources);
    await this.api.storage.session.set({ [tabKey(id)]: state });
    return state;
  }
  async handle(request: PopupRequest): Promise<PopupReply> {
    try {
      if (request.type === "status") {
        const result = await this.api.runtime.sendNativeMessage(NATIVE_HOST, {
          type: "hello",
        });
        if (!isNativeReply(result) || !result.ok)
          return {
            ok: false,
            code: isNativeReply(result)
              ? (result.errorCode ?? "BROWSER_NOT_CONNECTED")
              : "BROWSER_NOT_CONNECTED",
          };
        return { ok: true };
      }
      const tab = await this.current();
      let state = await this.read(tab.id);
      if (request.type === "scan") {
        try {
          state = await this.scan(tab.id, tab.url);
        } catch {
          return { ok: false, code: "PAGE_UNAVAILABLE" };
        }
      }
      if (!state || state.pageUrl !== tab.url)
        return { ok: false, code: "PAGE_UNAVAILABLE" };
      if (request.type === "capture") {
        const allowed = await this.api.permissions.contains({
          origins: ["http://*/*", "https://*/*"],
        });
        if (request.enabled && !allowed)
          return { ok: false, code: "PERMISSION_DENIED" };
        state.capturing = request.enabled;
        await this.api.storage.session.set({ [tabKey(tab.id)]: state });
      }
      if (request.type === "download") {
        const source = state.sources.find((s) => s.id === request.id);
        if (!source) return { ok: false, code: "SOURCE_EXPIRED" };
        if (!source.submitted) {
          const input = {
            kind: source.kind,
            url: source.url,
            pageUrl:
              new URL(state.pageUrl).origin + new URL(state.pageUrl).pathname,
            title: source.title,
          };
          const response = await this.api.runtime.sendNativeMessage(
            NATIVE_HOST,
            { type: "download", requestId: source.requestId, input },
          );
          if (!isNativeReply(response) || !response.ok || !response.taskId)
            return {
              ok: false,
              code: isNativeReply(response)
                ? (response.errorCode ?? "BROWSER_NOT_CONNECTED")
                : "BROWSER_NOT_CONNECTED",
            };
          source.submitted = true;
          await this.api.storage.session.set({ [tabKey(tab.id)]: state });
        }
      }
      return { ok: true, list: sourceView(state) };
    } catch (cause) {
      return {
        ok: false,
        code:
          cause instanceof Error && cause.message === "PAGE_UNAVAILABLE"
            ? "PAGE_UNAVAILABLE"
            : "BROWSER_NOT_CONNECTED",
      };
    }
  }
  async record(details: {
    tabId: number;
    url: string;
    mime: string;
    bytes?: number;
  }) {
    const state = await this.read(details.tabId);
    if (!state?.capturing) return;
    let tab: chrome.tabs.Tab;
    try {
      tab = await this.api.tabs.get(details.tabId);
    } catch {
      return;
    }
    if (tab.url !== state.pageUrl) return;
    const kind = sourceKind(details.url, details.mime);
    if (!kind) return;
    merge(state, [
      { url: details.url, kind, title: state.title, bytes: details.bytes },
    ]);
    await this.api.storage.session.set({ [tabKey(details.tabId)]: state });
  }
}
export function merge(state: TabSources, sources: FoundSource[]) {
  let size = state.sources.reduce(
    (sum, source) => sum + source.url.length + source.title.length * 2 + 128,
    0,
  );
  for (const source of sources.slice(0, 100)) {
    const url = publicUrl(source.url);
    if (
      !url ||
      !["hls", "video", "douyin"].includes(source.kind) ||
      state.sources.some((item) => item.url === url.href) ||
      state.sources.length >= 100
    )
      continue;
    size += url.href.length + source.title.length * 2 + 128;
    if (size > 131072) break;
    state.sources.push({
      url: url.href,
      kind: source.kind,
      title: source.title.trim().slice(0, 160) || state.title || "Video",
      bytes:
        typeof source.bytes === "number" &&
        Number.isSafeInteger(source.bytes) &&
        source.bytes > 0
          ? source.bytes
          : undefined,
      id: crypto.randomUUID(),
      requestId: crypto.randomUUID(),
      submitted: false,
    });
  }
  state.updatedAt = Date.now();
}
