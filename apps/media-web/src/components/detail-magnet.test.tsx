import { afterEach, beforeEach, expect, jest, mock, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import {
  browserImageLoader,
  createBrowserMediaClient,
  type MediaEntry,
} from "@assemble/media";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import samples from "../../../../packages/media/contract/snapshots.json";
import { button } from "../test-support/dom";
import { DetailViewer } from "./detail-viewer";

const magnet =
  "magnet:?xt=urn:btih:0123456789012345678901234567890123456789&dn=%E4%B8%AD%E6%96%87%3C%E7%94%B5%E5%BD%B1%3E&tr=https%3A%2F%2Ftracker.example.test%2Fannounce";
const entry = { ...samples.entries[0], magnetUri: magnet } as MediaEntry;
const client = createBrowserMediaClient("shared", {
  fetch: Object.assign(
    mock(async () =>
      Response.json({ entryId: entry.id, version: 1, items: [] }),
    ),
    { preconnect: () => {} },
  ),
});
let host: HTMLDivElement;
let root: Root;

beforeEach(() => {
  jest.useFakeTimers();
  setLanguage("zh-CN");
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => {
    root.unmount();
    setLanguage("zh-CN");
  });
  host.remove();
  jest.useRealTimers();
});
async function mount(
  onCopyMagnet: () => Promise<void>,
  item = entry,
  onSignIn?: () => void,
) {
  await act(async () =>
    root.render(
      <DetailViewer
        client={client}
        entry={item}
        imageLoader={browserImageLoader}
        sessionSignal={new AbortController().signal}
        onActor={() => {}}
        onCopyMagnet={onCopyMagnet}
        onSignIn={onSignIn}
      />,
    ),
  );
}

test("完整展示原始磁力，图标复制成功两秒后恢复，再次复制重新计时", async () => {
  const copy = mock(async () => {});
  await mount(copy);
  expect(host.querySelector("code")?.textContent).toBe(magnet);
  const control = button(host, "复制磁力链接");
  expect(control.textContent).toBe("");
  expect(control.querySelector(".lucide-copy")).not.toBeNull();
  await act(async () => control.click());
  expect(copy).toHaveBeenCalledTimes(1);
  expect(button(host, "已复制")).toBe(control);
  expect(control.disabled).toBe(false);
  expect(control.querySelector(".lucide-check")).not.toBeNull();
  const firstMessage = host.querySelector('[role="status"]')?.textContent;
  await act(async () => jest.advanceTimersByTime(1_500));
  await act(async () => control.click());
  expect(copy).toHaveBeenCalledTimes(2);
  expect(host.querySelector('[role="status"]')?.textContent).not.toBe(
    firstMessage,
  );
  await act(async () => jest.advanceTimersByTime(500));
  expect(control.getAttribute("aria-label")).toBe("已复制");
  await act(async () => jest.advanceTimersByTime(1_499));
  expect(control.querySelector(".lucide-check")).not.toBeNull();
  await act(async () => jest.advanceTimersByTime(1));
  expect(button(host, "复制磁力链接")).toBe(control);
  expect(control.querySelector(".lucide-copy")).not.toBeNull();
  expect(host.querySelector('[role="status"]')?.textContent).toBe("");
});

test("英文图标有可访问名称，失败保留完整链接和固定提示，可再次复制", async () => {
  setLanguage("en");
  let fail = true;
  await mount(async () => {
    if (fail) throw new Error("untrusted-diagnostic-<secret>");
  });
  const control = button(host, "Copy magnet link");
  await act(async () => control.click());
  expect(host.querySelector("code")?.textContent).toBe(magnet);
  expect(host.querySelector('[role="alert"]')?.textContent).toContain(
    "Copy failed",
  );
  expect(host.textContent).not.toContain("untrusted-diagnostic");
  expect(control.querySelector(".lucide-check")).toBeNull();
  fail = false;
  await act(async () => control.click());
  expect(button(host, "Copied")).toBe(control);
  expect(host.querySelector('[role="alert"]')).toBeNull();
  await act(async () => jest.advanceTimersByTime(2_000));
  expect(button(host, "Copy magnet link")).toBe(control);
});

test("匿名详情仅保留登录入口，不展示磁力文本或复制按钮", async () => {
  const copy = mock(async () => {});
  const signIn = mock(() => {});
  await mount(copy, { ...entry, magnetUri: null }, signIn);
  expect(host.querySelector(".detail-magnet")).toBeNull();
  expect(host.textContent).not.toContain("magnet:?");
  await act(async () => button(host, "登录后查看磁力链接").click());
  expect(signIn).toHaveBeenCalledTimes(1);
  expect(copy).not.toHaveBeenCalled();
});

test("卸载后迟到的复制完成不创建反馈计时器", async () => {
  let finish: (() => void) | undefined;
  await mount(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
  );
  await act(async () => button(host, "复制磁力链接").click());
  await act(async () => root.render(null));
  const pending = jest.getTimerCount();
  await act(async () => finish?.());
  expect(jest.getTimerCount()).toBe(pending);
});
