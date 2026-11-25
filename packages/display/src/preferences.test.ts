import { describe, expect, test } from "bun:test";
import { runInNewContext } from "node:vm";
import { bootstrapDisplay, DISPLAY_STORAGE_KEY } from "./bootstrap";
import { createDisplayStore } from "./preferences";

function environment(stored: string | null = null, dark = false) {
  const values = new Map<string, string>();
  if (stored !== null) values.set(DISPLAY_STORAGE_KEY, stored);
  let blocked = false;
  const storage: Storage = {
    get length() {
      return values.size;
    },
    key: (index) => [...values.keys()][index] ?? null,
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => {
      if (blocked) throw new Error("storage blocked");
      values.set(key, value);
    },
    removeItem: (key) => {
      values.delete(key);
    },
    clear: () => {
      values.clear();
    },
  };
  const media = Object.assign(new EventTarget(), { matches: dark });
  const events = new EventTarget();
  const win = {
    get localStorage() {
      if (blocked) throw new Error("storage blocked");
      return storage;
    },
    matchMedia: () => media,
    addEventListener: events.addEventListener.bind(events),
    removeEventListener: events.removeEventListener.bind(events),
  } as unknown as Window;
  const doc = document.implementation.createHTMLDocument();
  const meta = doc.createElement("meta");
  meta.name = "color-scheme";
  meta.content = "light dark";
  doc.head.append(meta);
  return {
    win,
    doc,
    storage,
    values,
    block() {
      blocked = true;
    },
    system(value: boolean) {
      media.matches = value;
      media.dispatchEvent(new Event("change"));
    },
    sync(value: string | null, key: string | null = DISPLAY_STORAGE_KEY) {
      if (value === null) values.delete(DISPLAY_STORAGE_KEY);
      else values.set(DISPLAY_STORAGE_KEY, value);
      events.dispatchEvent(new StorageEvent("storage", { key }));
    },
  };
}

describe("共享语言与外观偏好", () => {
  test("首次跟随系统；明确选择即使暂时与系统相同也保留", () => {
    const env = environment(null, true);
    const store = createDisplayStore(env.win, env.doc);
    expect(store.getDisplayPreferences()).toEqual({
      language: "zh-CN",
      theme: "system",
      resolvedTheme: "dark",
    });
    env.system(false);
    expect(env.doc.documentElement.className).toBe("light");
    store.setTheme("dark");
    env.system(true);
    env.system(false);
    expect(store.getDisplayPreferences().resolvedTheme).toBe("dark");
    expect(env.storage.getItem(DISPLAY_STORAGE_KEY)).toBe('{"theme":"dark"}');
    store.setLanguage("en");
    store.setTheme("system");
    expect(env.doc.documentElement.lang).toBe("en");
    expect(env.doc.documentElement.style.colorScheme).toBe("light dark");
    expect(
      env.doc
        .querySelector('meta[name="color-scheme"]')
        ?.getAttribute("content"),
    ).toBe("light dark");
    expect(env.storage.getItem(DISPLAY_STORAGE_KEY)).toBe('{"language":"en"}');
    env.system(true);
    expect(store.getDisplayPreferences().resolvedTheme).toBe("dark");
    store.setLanguage("zh-CN");
    expect(env.storage.getItem(DISPLAY_STORAGE_KEY)).toBeNull();
    store.dispose();
  });

  test("刷新恢复；同源窗口收到其他窗口修改与重置，且不写回", () => {
    const env = environment('{"language":"en","theme":"light"}', true);
    const store = createDisplayStore(env.win, env.doc);
    let updates = 0;
    store.subscribeDisplayPreferences(() => {
      updates++;
    });
    expect(store.getDisplayPreferences().resolvedTheme).toBe("light");
    env.sync('{"language":"zh-CN","theme":"dark"}');
    expect(store.getDisplayPreferences().language).toBe("zh-CN");
    expect(env.doc.documentElement.className).toBe("dark");
    expect(
      env.doc
        .querySelector('meta[name="color-scheme"]')
        ?.getAttribute("content"),
    ).toBe("dark");
    env.sync('{"language":"en"}', "unrelated");
    expect(updates).toBe(1);
    env.sync(null, null);
    expect(store.getDisplayPreferences()).toEqual({
      language: "zh-CN",
      theme: "system",
      resolvedTheme: "dark",
    });
    expect(updates).toBe(2);
    expect(env.values.size).toBe(0);
    store.dispose();
  });

  test("损坏、未知值和禁用存储不阻止启动或在内存中切换", () => {
    for (const value of [
      "broken",
      "[]",
      "null",
      '{"language":"fr","theme":"purple"}',
    ]) {
      const env = environment(value);
      const store = createDisplayStore(env.win, env.doc);
      expect(store.getDisplayPreferences().language).toBe("zh-CN");
      expect(store.getDisplayPreferences().theme).toBe("system");
      store.dispose();
    }
    const env = environment();
    env.block();
    const store = createDisplayStore(env.win, env.doc);
    store.setLanguage("en");
    store.setTheme("dark");
    expect(env.doc.documentElement.lang).toBe("en");
    expect(env.doc.documentElement.className).toBe("dark");
    store.dispose();
  });

  test("首屏脚本独立执行，不依赖模块变量或 React", () => {
    const env = environment('{"language":"en","theme":"dark"}');
    const source = `(${bootstrapDisplay.toString()})(window,document,${JSON.stringify(DISPLAY_STORAGE_KEY)})`;
    runInNewContext(source, { window: env.win, document: env.doc });
    expect(env.doc.documentElement.lang).toBe("en");
    expect(env.doc.documentElement.className).toBe("dark");
  });
});
