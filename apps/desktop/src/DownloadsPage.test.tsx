import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import { BridgeError } from "@/lib/bridge";
import { downloadError } from "@/lib/download-i18n";
import {
  type DownloadTask,
  downloadProgress,
  isDownloadsView,
} from "@/lib/downloads";
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
async function chooseMagnet(dialog: HTMLElement) {
  const trigger = dialog.querySelector<HTMLElement>('[role="combobox"]');
  await act(async () =>
    trigger?.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
    ),
  );
  await settle();
  await click(
    [...document.querySelectorAll<HTMLElement>('[role="option"]')].find(
      (v) => v.textContent?.trim() === "磁力链",
    ),
  );
}
test("Mac 直连默认关闭，明确勾选后只随本次磁力预览提交并在开始后重置", async () => {
  const source = new FakeBridge(snapshot());
  source.downloadPreview = {
    id: "p",
    kind: "magnet",
    magnetNetwork: "direct",
    title: "公开样本",
    sourceHost: "BitTorrent",
    durationSeconds: null,
    files: [{ id: 0, name: "sample.bin", size: 1 }],
    variants: [{ id: "magnet", width: null, height: null, bandwidth: null }],
  };
  const view = await render(
    <DownloadsPage
      source={source}
      view={{ ...source.current.downloads, directBtAvailable: true }}
    />,
  );
  await click(button(view.host, "新建下载"));
  let dialog = currentDialog();
  await chooseMagnet(dialog);
  const direct = dialog.querySelector<HTMLElement>('[id$="-direct-bt"]');
  expect(direct?.getAttribute("aria-checked")).toBe("false");
  expect(text(dialog)).toContain("真实出口 IP");
  await click(direct ?? undefined);
  await typeInto(dialog, `magnet:?xt=urn:btih:${"a".repeat(40)}`);
  await click(button(dialog, "解析地址"));
  expect(source.downloadInput?.magnetNetwork).toBe("direct");
  await click(button(dialog, "开始下载"));
  await click(button(view.host, "新建下载"));
  dialog = currentDialog();
  expect(
    dialog.querySelector('[id$="-direct-bt"]')?.getAttribute("aria-checked"),
  ).toBe("false");
  await act(async () => setLanguage("en"));
  expect(text(dialog)).toContain("real outbound IP");
});

test("助手任务标记跟随语言，普通任务不增加标记", async () => {
  const source = new FakeBridge(snapshot());
  const task: DownloadTask = {
    id: "assistant-task",
    initiatedBy: "assistant",
    kind: "hls",
    title: "用户的中文视频 <film>",
    fileName: "用户的中文视频 <film>.mp4",
    sourceHost: "example.com",
    phase: "completed",
    receivedBytes: 1024,
    totalBytes: 1024,
    completedSegments: 1,
    totalSegments: 1,
    bytesPerSecond: 0,
    mediaSeconds: 10,
    durationSeconds: 10,
    errorCode: null,
    retryable: false,
    createdAt: "2026-11-27T01:00:00Z",
  };
  const downloads = { ...source.current.downloads, tasks: [task] };
  expect(isDownloadsView(downloads)).toBe(true);
  expect(
    isDownloadsView({
      ...downloads,
      tasks: [{ ...task, initiatedBy: "unknown" }],
    }),
  ).toBe(false);
  expect(isDownloadsView({ ...downloads, assistantAllowed: "false" })).toBe(
    false,
  );
  const view = await render(<DownloadsPage source={source} view={downloads} />);
  expect(text(view.host)).toContain("由助手发起");
  expect(text(view.host)).not.toContain("已保存");
  expect(text(view.host)).not.toContain("资源 1 / 1");
  expect(text(view.host)).toContain("用户的中文视频 <film>.mp4");
  await act(async () => setLanguage("en"));
  expect(text(view.host)).toContain("Started by assistant");
  expect(text(view.host)).not.toContain("由助手发起");
  expect(view.host.querySelector("film")).toBeNull();
  await view.unmount();
  const normal = { ...task };
  delete normal.initiatedBy;
  const regular = await render(
    <DownloadsPage source={source} view={{ ...downloads, tasks: [normal] }} />,
  );
  expect(text(regular.host)).not.toContain("Started by assistant");
});
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

test("磁力文件选择不依赖 FFmpeg，开始只提交选中的文件标识", async () => {
  const source = new FakeBridge(snapshot());
  source.current.downloads.toolsAvailable = false;
  source.downloadPreview = {
    id: "magnet-preview",
    kind: "magnet",
    title: "公开资料",
    sourceHost: "BitTorrent",
    durationSeconds: null,
    variants: [{ id: "magnet", width: null, height: null, bandwidth: null }],
    files: [
      { id: 0, name: "目录/视频.bin", size: 1024 },
      { id: 2, name: "说明.txt", size: 10 },
    ],
  };
  let selection: number[] | undefined;
  source.downloadStart = async (_id, _variant, ids) => {
    selection = ids;
    return "task";
  };
  const view = await render(
    <DownloadsPage source={source} view={source.current.downloads} />,
  );
  await click(button(view.host, "新建下载"));
  const dialog = currentDialog();
  const trigger = dialog.querySelector<HTMLElement>('[role="combobox"]');
  await act(async () =>
    trigger?.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
    ),
  );
  await settle();
  const option = [
    ...document.querySelectorAll<HTMLElement>('[role="option"]'),
  ].find((v) => v.textContent?.trim() === "磁力链");
  await click(option);
  expect(text(dialog)).toContain("上传限速 256 KiB/秒");
  await typeInto(dialog, `magnet:?xt=urn:btih:${"a".repeat(40)}`);
  await click(button(dialog, "解析地址"));
  expect(source.downloadInput?.kind).toBe("magnet");
  expect(dialog.querySelectorAll('[role="checkbox"]')).toHaveLength(2);
  await click(button(dialog, "取消全选"));
  expect(button(dialog, "开始下载")?.disabled).toBe(true);
  const checkbox = dialog.querySelector<HTMLElement>('[role="checkbox"]');
  await click(checkbox ?? undefined);
  expect(button(dialog, "开始下载")?.disabled).toBe(false);
  await click(button(dialog, "开始下载"));
  expect(selection).toEqual([0]);
});
