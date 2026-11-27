import { expect, test } from "bun:test";
import { entryContent, validMediaContent } from "./editor";

test("日期手动输入不允许无效日历日期，闰日和较早日期保持原值", () => {
  const content = { ...entryContent(), title: "日期校验" };
  for (const key of ["releaseDate", "watchedAt"] as const) {
    for (const date of [
      "2026-02-30",
      "2026-02-29",
      "2026-13-01",
      "2026-11-",
      "2026-11-25T00:00:00Z",
    ]) {
      expect(validMediaContent({ ...content, [key]: date })).toBe(false);
    }
    for (const date of [null, "1897-08-01", "2024-02-29", "2026-11-25"]) {
      expect(validMediaContent({ ...content, [key]: date })).toBe(true);
    }
  }
});
