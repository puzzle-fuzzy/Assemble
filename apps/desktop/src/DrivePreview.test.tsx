import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import type { PreviewResource } from "@assemble/ui/lib/file-preview";
import { act } from "react";
import { DrivePreviewAction } from "./DrivePreview";
import { button, cleanup, click, render, settle } from "./test-support/dom";
import { FakeBridge, snapshot } from "./test-support/fakes";

afterEach(async () => {
  await cleanup();
  setLanguage("zh-CN");
});
const file = {
  id: "file-1",
  name: "中文<原件>.txt",
  size: 12,
  createdAt: "2026-11-27T01:00:00Z",
};

test("预览保留用户文字，双语切换只翻译系统文案，关闭取消读取并返回触发按钮", async () => {
  const source = new FakeBridge(snapshot({ phase: "signedIn" }));
  let signal: AbortSignal | null = null;
  let disposed = 0;
  source.drivePreview = async (_id, abort) => {
    signal = abort;
    return {
      preview: {
        kind: "text",
        mimeType: "text/plain; charset=utf-8",
        size: 2_000_000,
        text: "<script>用户的中文🌙</script>",
        truncated: true,
      },
      src: null,
      dispose: () => {
        disposed++;
      },
    };
  };
  const view = await render(
    <DrivePreviewAction source={source} file={file} disabled={false} />,
  );
  const trigger = button(view.host, "预览");
  if (!trigger) throw new Error("preview trigger missing");
  trigger.focus();
  await click(trigger);
  const dialog = document.querySelector('[role="dialog"]') as HTMLElement;
  expect(dialog.textContent).toContain(file.name);
  expect(dialog.querySelector("pre")?.textContent).toBe(
    "<script>用户的中文🌙</script>",
  );
  expect(dialog.querySelector("script")).toBeNull();
  expect(dialog.textContent).toContain("仅显示前 1 MiB");
  await act(async () => setLanguage("en"));
  expect(dialog.textContent).toContain("Only the first 1 MiB");
  expect(dialog.textContent).toContain(file.name);
  await click(button(dialog, "Close"));
  await settle();
  expect((signal as AbortSignal | null)?.aborted).toBe(true);
  expect(disposed).toBe(1);
  expect(document.activeElement).toBe(trigger);
});

test("关闭后才到达的结果会释放，错误不回显 Rust 诊断", async () => {
  const source = new FakeBridge(snapshot({ phase: "signedIn" }));
  let resolve!: (value: PreviewResource) => void;
  let disposed = 0;
  source.drivePreview = () =>
    new Promise((done) => {
      resolve = done;
    });
  const view = await render(
    <DrivePreviewAction source={source} file={file} disabled={false} />,
  );
  await click(button(view.host, "预览"));
  await click(
    button(document.querySelector('[role="dialog"]') as HTMLElement, "关闭"),
  );
  await act(async () =>
    resolve({
      preview: {
        kind: "unsupported",
        mimeType: null,
        size: 0,
        text: null,
        truncated: false,
      },
      src: null,
      dispose: () => {
        disposed++;
      },
    }),
  );
  expect(disposed).toBe(1);
  source.drivePreview = async () => {
    throw new Error("Rust credential diagnostic");
  };
  await click(button(view.host, "预览"));
  const dialog = document.querySelector('[role="dialog"]') as HTMLElement;
  expect(dialog.textContent).toContain("暂时无法预览");
  expect(dialog.textContent).not.toContain("credential");
  expect(button(dialog, "取回")).toBeDefined();
  await act(async () => setLanguage("en"));
  expect(dialog.textContent).toContain("This file could not be previewed");
  source.drivePreview = async () => ({
    preview: {
      kind: "unsupported",
      mimeType: null,
      size: 4,
      text: null,
      truncated: false,
    },
    src: null,
  });
  await click(button(dialog, "Retry"));
  expect(dialog.textContent).toContain("Preview is unavailable for this file");
  expect(button(dialog, "Download")).toBeDefined();
});
