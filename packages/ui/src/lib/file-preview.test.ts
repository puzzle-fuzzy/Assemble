import { expect, test } from "bun:test";
import samples from "../../../../apps/desktop/contract/previews.json";
import { isFilePreview } from "./file-preview";

test("Rust 与 TypeScript 使用同一份预览线路样本", () => {
  expect(samples).toHaveLength(5);
  for (const sample of samples) expect(isFilePreview(sample)).toBe(true);
});
test("预览契约拒绝活动内容、超限文本、额外凭据和畸形状态", () => {
  const value = {
    kind: "text",
    mimeType: "text/plain; charset=utf-8",
    size: 12,
    text: "中文<原文>",
    truncated: false,
  };
  expect(isFilePreview(value)).toBe(true);
  for (const invalid of [
    { ...value, mimeType: "text/html" },
    { ...value, token: "private" },
    { ...value, text: "中".repeat(400_000) },
    { ...value, kind: "unsupported" },
    { ...value, kind: "toString" },
    { ...value, size: -1 },
  ])
    expect(isFilePreview(invalid)).toBe(false);
});
