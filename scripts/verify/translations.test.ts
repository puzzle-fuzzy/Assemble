import { expect, test } from "bun:test";

function entries(value: unknown, prefix = ""): Map<string, string> {
  const result = new Map<string, string>();
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("translation dictionary must be an object");
  for (const [key, child] of Object.entries(value)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof child === "string") result.set(path, child);
    else
      for (const [nestedKey, text] of entries(child, path))
        result.set(nestedKey, text);
  }
  return result;
}

function baseKey(key: string) {
  return key.replace(/_(zero|one|two|few|many|other)$/, "");
}
function variables(text: string) {
  return [...text.matchAll(/\{\{\s*([^},\s]+)[^}]*\}\}/g)]
    .map((match) => match[1])
    .sort();
}

// 英语复数可以有多个分支，但每个分支都必须与中文的变量契约对应。
for (const [name, directory, zh, en] of [
  ["管理端", "apps/admin/src/locales/", "zh-CN.json", "en.json"],
  ["桌面界面", "apps/desktop/src/locales/", "zh-CN.json", "en.json"],
  [
    "桌面辅助文案",
    "apps/desktop/src/locales/",
    "text.zh-CN.json",
    "text.en.json",
  ],
] as const) {
  test(`${name}中英字典的键、插值与复数分支完整`, async () => {
    const root = new URL("../../", import.meta.url);
    const source = entries(
      await Bun.file(new URL(directory + zh, root)).json(),
    );
    const target = entries(
      await Bun.file(new URL(directory + en, root)).json(),
    );
    expect([...new Set([...source.keys()].map(baseKey))].sort()).toEqual(
      [...new Set([...target.keys()].map(baseKey))].sort(),
    );
    for (const [key, text] of target) {
      expect(text.trim().length, key).toBeGreaterThan(0);
      const reference =
        source.get(key) ??
        source.get(baseKey(key)) ??
        source.get(`${baseKey(key)}_other`);
      expect(reference, key).toBeDefined();
      expect(variables(text), key).toEqual(variables(reference ?? ""));
      if (key.endsWith("_one"))
        expect(target.has(`${baseKey(key)}_other`), key).toBe(true);
    }
  });
}
