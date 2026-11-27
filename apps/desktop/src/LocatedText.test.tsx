import { expect, test } from "bun:test";
import { locatedSegments } from "./LocatedText";

test("长地名优先，保留 URL 和原文，不会把网址中的地名变成另一条链接", () => {
  const places = [
    { label: "上海东方艺术中心", address: "上海丁香路425号" },
    { label: "东方艺术中心", address: "上海丁香路425号" },
  ];
  const original =
    "上海东方艺术中心→东方艺术中心\nhttps://example.test/东方艺术中心 原文";
  const segments = locatedSegments(original, places);
  expect(segments.map((s) => s.text).join("")).toBe(original);
  expect(segments.filter((s) => s.location).map((s) => s.text)).toEqual([
    "上海东方艺术中心",
    "东方艺术中心",
  ]);
  expect(locatedSegments("未绑定的地名", places)).toEqual([
    { text: "未绑定的地名" },
  ]);
});
