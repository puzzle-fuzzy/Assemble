import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import { BridgeError } from "@/lib/bridge";
import { downloadError } from "@/lib/download-i18n";
import { type DownloadTask, downloadProgress } from "@/lib/downloads";
import {
  button,
  cleanup,
  click,
  render,
  settle,
  text,
  textField,
  typeInto,
} from "@/test-support/dom";
import { FakeBridge, snapshot } from "@/test-support/fakes";
import { renderApp } from "@/test-support/render-app";
import { DownloadsPage } from "./DownloadsPage";

afterEach(async () => {
  await cleanup();
  setLanguage("zh-CN");
});
function currentDialog() {
  const dialog = document.querySelector<HTMLElement>('[role="dialog"]');
  if (!dialog) throw new Error("download dialog missing");
  return dialog;
}
test("未登录可以进入下载页，解析后选择画质并下载", async () => {
  const source = new FakeBridge(snapshot());
  const view = await renderApp(source);
  await click(button(view.host, "网络文件下载"));
  expect(text(view.host)).toContain("还没有下载任务");
  expect(text(view.host)).not.toContain("在浏览器完成登录");
  expect(document.querySelector('[role="dialog"]')).toBeNull();
  await click(button(view.host, "新建下载"));
  const dialog = currentDialog();
  const input = textField(dialog);
  expect(document.activeElement).toBe(input);
  await typeInto(dialog, "https://example.com/a.m3u8?signature=private");
  await click(button(dialog, "解析地址"));
  await settle();
  expect(source.downloadInput?.url).toContain("signature=private");
  expect(input.value).toBe("");
  await click(button(dialog, "开始下载"));
  expect(source.calls).toContain("downloadStart");
  expect(document.querySelector('[role="dialog"]')).toBeNull();
});
test("英文错误只显示本地文案，切换语言保留输入与用户内容", async () => {
  const source = new FakeBridge(snapshot());
  source.failWith = new BridgeError(
    "DOWNLOAD_NETWORK",
    "Rust: secret diagnostic",
  );
  const view = await render(
    <DownloadsPage source={source} view={source.current.downloads} />,
  );
  await click(button(view.host, "新建下载"));
  const dialog = currentDialog();
  const input = textField(dialog);
  await typeInto(dialog, "https://example.com/中文.m3u8");
  await click(button(dialog, "解析地址"));
  await settle();
  expect(text(dialog)).toContain("网络连接失败");
  await act(async () => {
    setLanguage("en");
  });
  await settle();
  expect(text(dialog)).toContain("The network connection failed");
  expect(text(dialog)).not.toContain("secret diagnostic");
  expect(input.value).toContain("中文");
  expect(downloadError("Rust-private-detail")).toBe(
    "The operation did not complete. Try again.",
  );
});
test("选择本地 M3U8 先直接读取，缺少链接时再提示并复用同一个地址框", async () => {
  const source = new FakeBridge(snapshot());
  const imported: (string | null)[] = [];
  source.downloadImportHls = async (link) => {
    imported.push(link);
    if (!link)
      throw new BridgeError("DOWNLOAD_INVALID_SOURCE", "Rust: private detail");
    return source.downloadPreview;
  };
  const view = await render(
    <DownloadsPage source={source} view={source.current.downloads} />,
  );
  await click(button(view.host, "新建下载"));
  const dialog = currentDialog();
  const input = textField(dialog);
  expect(dialog.querySelectorAll("input")).toHaveLength(2);
  expect(text(dialog)).toContain("M3U8 下载链接");
  expect(text(dialog)).not.toContain("原始地址");
  expect(text(dialog)).not.toContain("相对分片");
  expect(dialog.querySelector('[role="alert"]')).toBeNull();
  await click(button(dialog, "选择 M3U8 文件"));
  await settle();
  expect(imported).toEqual([null]);
  expect(text(dialog)).toContain("粘贴下载该 M3U8 文件的链接");
  expect(text(dialog)).toContain("浏览器插件");
  expect(text(dialog)).not.toContain("private detail");
  expect(input.getAttribute("aria-invalid")).toBe("true");
  expect(
    document
      .getElementById(input.getAttribute("aria-describedby") ?? "")
      ?.getAttribute("role"),
  ).toBe("alert");
  expect(document.activeElement).toBe(input);
  await act(async () => setLanguage("en"));
  expect(text(dialog)).toContain("link used to download the M3U8 file");
  await typeInto(dialog, "  https://example.com/中文/index.m3u8  ");
  expect(dialog.querySelector('[role="alert"]')).toBeNull();
  expect(input.getAttribute("aria-invalid")).toBe("false");
  expect(input.hasAttribute("aria-describedby")).toBe(false);
  await click(button(dialog, "Choose M3U8 file"));
  expect(imported).toEqual([null, "https://example.com/中文/index.m3u8"]);
  expect(text(dialog)).toContain("Start download");
});

test("本地文件已有完整链接时直接解析，不额外索要下载地址", async () => {
  const source = new FakeBridge(snapshot());
  let received: string | null | undefined;
  source.downloadImportHls = async (link) => {
    received = link;
    return source.downloadPreview;
  };
  const view = await render(
    <DownloadsPage source={source} view={source.current.downloads} />,
  );
  await click(button(view.host, "新建下载"));
  const dialog = currentDialog();
  await click(button(dialog, "选择 M3U8 文件"));
  expect(received).toBeNull();
  expect(text(dialog)).toContain("开始下载");
  expect(dialog.querySelector('[role="alert"]')).toBeNull();
});
test("浏览器的新任务定位到下载页，普通进度更新不抢导航", async () => {
  const source = new FakeBridge(snapshot());
  const view = await renderApp(source);
  await act(async () => {
    source.push(
      snapshot({
        downloads: { ...source.current.downloads, browserTaskId: "browser-1" },
      }),
    );
  });
  await settle();
  expect(document.title).toBe("网络文件下载 · Assemble");
  await click(button(view.host, "本机设置"));
  await act(async () => {
    source.push({ ...source.current });
  });
  await settle();
  expect(document.title).toBe("本机设置 · Assemble");
  await act(async () => {
    source.push(
      snapshot({
        downloads: { ...source.current.downloads, browserTaskId: "browser-2" },
      }),
    );
  });
  await settle();
  expect(document.title).toBe("网络文件下载 · Assemble");
});

test("未知长度保持不定进度，下载和合并只使用真实计数", () => {
  const task = {
    phase: "downloading",
    totalSegments: null,
    totalBytes: null,
    mediaSeconds: null,
    durationSeconds: null,
    receivedBytes: 50,
    completedSegments: 2,
  } as DownloadTask;
  expect(downloadProgress(task)).toBeNull();
  expect(downloadProgress({ ...task, totalSegments: 8 })).toBe(25);
  expect(
    downloadProgress({
      ...task,
      phase: "muxing",
      durationSeconds: 10,
      mediaSeconds: 3,
    }),
  ).toBe(30);
  expect(downloadProgress({ ...task, phase: "verifying" })).toBeNull();
});
