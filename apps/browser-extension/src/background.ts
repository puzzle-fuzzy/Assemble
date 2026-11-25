import { Controller, tabKey } from "./controller";
import { type PopupRequest, sourceKind, type TabSources } from "./protocol";

const controller = new Controller(chrome);
// 这里只串行化存储修改；真数据始终在 session，worker 重启不会丢失捕获记录。
let turn = Promise.resolve();
let queuedResources = 0;
async function serial<T>(work: () => Promise<T>): Promise<T> {
  const previous = turn;
  let release = () => {};
  turn = new Promise<void>((resolve) => {
    release = resolve;
  });
  await previous;
  try {
    return await work();
  } finally {
    release();
  }
}
function valid(value: unknown): value is PopupRequest {
  if (typeof value !== "object" || !value) return false;
  const message = value as Record<string, unknown>;
  return (
    ["scan", "list", "status"].includes(String(message.type)) ||
    (message.type === "capture" && typeof message.enabled === "boolean") ||
    (message.type === "download" &&
      typeof message.id === "string" &&
      message.id.length <= 64)
  );
}
chrome.runtime.onMessage.addListener((message: unknown, sender, respond) => {
  if (sender.id !== chrome.runtime.id || sender.tab || !valid(message))
    return false;
  void (async () => {
    try {
      respond(await serial(() => controller.handle(message)));
    } catch {
      respond({ ok: false, code: "BROWSER_NOT_CONNECTED" });
    }
  })();
  return true;
});
chrome.webRequest.onHeadersReceived.addListener(
  (details) => {
    if (
      details.tabId < 0 ||
      details.statusCode < 200 ||
      details.statusCode >= 300
    )
      return;
    const mime =
      details.responseHeaders?.find(
        (h) => h.name.toLowerCase() === "content-type",
      )?.value ?? "";
    const length = Number(
      details.responseHeaders?.find(
        (h) => h.name.toLowerCase() === "content-length",
      )?.value,
    );
    if (!sourceKind(details.url, mime) || queuedResources >= 128) return;
    queuedResources++;
    void (async () => {
      try {
        await serial(() =>
          controller.record({
            tabId: details.tabId,
            url: details.url,
            mime,
            bytes: Number.isSafeInteger(length) ? length : undefined,
          }),
        );
      } catch {
        /* 标签关闭或存储已清除时丢弃资源，不记录地址与浏览器诊断。 */
      } finally {
        queuedResources--;
      }
    })();
  },
  {
    urls: ["http://*/*", "https://*/*"],
    types: ["media", "xmlhttprequest", "other"],
  },
  ["responseHeaders"],
);
chrome.tabs.onRemoved.addListener((id) => {
  void serial(() => chrome.storage.session.remove(tabKey(id)));
});
chrome.tabs.onUpdated.addListener((id, change) => {
  if (change.status !== "loading") return;
  void serial(async () => {
    const state = await controller.read(id);
    if (!state) return;
    let tab: chrome.tabs.Tab;
    try {
      tab = await chrome.tabs.get(id);
    } catch {
      await chrome.storage.session.remove(tabKey(id));
      return;
    }
    if (!tab.url || new URL(tab.url).origin !== new URL(state.pageUrl).origin) {
      await chrome.storage.session.remove(tabKey(id));
      return;
    }
    state.pageUrl = tab.url;
    state.sources = [];
    state.updatedAt = Date.now();
    await chrome.storage.session.set({ [tabKey(id)]: state });
  });
});
chrome.permissions.onRemoved.addListener(() => {
  void serial(async () => {
    const all = await chrome.storage.session.get(null);
    for (const [key, item] of Object.entries(all)) {
      const value = item as TabSources | undefined;
      if (key.startsWith("sources:") && value?.capturing) {
        value.capturing = false;
        await chrome.storage.session.set({ [key]: value });
      }
    }
  });
});
