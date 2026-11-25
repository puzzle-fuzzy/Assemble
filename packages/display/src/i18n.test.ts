import { afterEach, expect, test } from "bun:test";
import { i18n, registerTranslations, setLanguage } from "./index";

afterEach(() => {
  setLanguage("zh-CN");
});

test("切换使用完整句子、复数和插值，用户内容保持原样", () => {
  registerTranslations("displayTest", {
    "zh-CN": { files: "{{name}}：{{count}} 个文件" },
    en: {
      files_one: "{{name}}: {{count}} file",
      files_other: "{{name}}: {{count}} files",
    },
  });
  const t = (count: number) =>
    i18n.t("files", { ns: "displayTest", name: "旅行 <2026>", count });
  expect(t(1)).toBe("旅行 <2026>：1 个文件");
  setLanguage("en");
  expect(t(1)).toBe("旅行 <2026>: 1 file");
  expect(t(2)).toBe("旅行 <2026>: 2 files");
  setLanguage("zh-CN");
  expect(t(2)).toBe("旅行 <2026>：2 个文件");
});
