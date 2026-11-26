import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import type { SystemNotification } from "@assemble/ui/lib/system-notifications";
import { act } from "react";
import { App } from "./App";
import { BridgeError } from "./lib/bridge";
import { DesktopSystemNotifications } from "./SystemNotifications";
import { cleanup, click, render, settle } from "./test-support/dom";
import { FakeBridge, snapshot, transfer } from "./test-support/fakes";

afterEach(async () => {
  await cleanup();
  setLanguage("zh-CN");
});

test("本机通知同步失败会自动重试，恢复时保持弹窗打开且不提供刷新按钮", async () => {
  const source = new FakeBridge(snapshot());
  source.failWith = new BridgeError(
    "notification_store_unavailable",
    "private Rust disk path",
  );
  await render(
    <DesktopSystemNotifications
      source={source}
      view={{ items: [], available: true }}
      onNavigate={() => {}}
    />,
  );
  await act(async () => window.dispatchEvent(new Event("focus")));
  await settle();
  await click(button("通知"));
  const dialog = document.querySelector('[role="dialog"]');
  expect(dialog?.textContent).toContain("正在自动重试");
  source.failWith = null;
  await act(async () => window.dispatchEvent(new Event("focus")));
  await settle();
  expect(document.querySelector('[role="dialog"]')).toBe(dialog);
  expect(dialog?.textContent).toContain("暂无系统通知");
  expect(dialog?.textContent).not.toContain("正在自动重试");
  expect(dialog?.textContent).not.toContain("刷新");
  expect(
    source.calls.filter((call) => call === "systemNotificationsRefresh"),
  ).toHaveLength(2);
});
const notice: SystemNotification = {
  id: "system-file",
  revision: 2,
  content: {
    type: "transferFailed",
    transferId: "t1",
    name: "中文 <合同>.pdf",
    localDiskFull: true,
  },
  level: "error",
  createdAt: "2026-11-26T01:00:00.000Z",
  updatedAt: "2026-11-26T01:00:00.000Z",
  resolvedAt: null,
  readAt: null,
};
function button(name: string) {
  const found = [
    ...document.querySelectorAll<HTMLButtonElement>("button"),
  ].find(
    (item) =>
      (item.getAttribute("aria-label") ?? item.textContent?.trim()) === name,
  );
  if (!found) throw new Error(`missing button ${name}`);
  return found;
}

test("桌面共用铃铛，前往通知目标保留原文件名并聚焦真实任务；已读以Rust快照为准", async () => {
  const initial = snapshot({
    phase: "signedIn",
    transfers: [
      transfer({
        name: "中文 <合同>.pdf",
        phase: "failed",
        failure: "diskFull",
      }),
    ],
    systemNotifications: { items: [notice], available: true },
  });
  const source = new FakeBridge(initial);
  await render(<App bridge={source} />);
  expect(document.querySelector('[role="dialog"]')).toBeNull();
  await click(button("通知，有未读消息"));
  expect(document.body.textContent).toContain("本机存储空间不足");
  expect(document.body.textContent).toContain("中文 <合同>.pdf");
  expect(document.querySelector('[data-slot="dialog-content"]')).toBeTruthy();
  expect(document.querySelector('[role="dialog"]')?.textContent).not.toContain(
    "刷新",
  );
  await click(button("查看传输"));
  await settle();
  expect(source.calls).toContain("systemNotificationsRead:system-file");
  expect(document.activeElement?.id).toBe("transfer-t1");
  expect(button("通知，有未读消息")).toBeTruthy();
  await act(async () =>
    source.push({
      ...initial,
      systemNotifications: {
        items: [{ ...notice, readAt: "2026-11-26T01:01:00.000Z" }],
        available: true,
      },
    }),
  );
  expect(button("通知")).toBeTruthy();
});

test("本机保存已读失败时保留未读，错误只用本地化文案；过期任务历史不提供失效跳转", async () => {
  const source = new FakeBridge(
    snapshot({
      phase: "signedIn",
      systemNotifications: { items: [notice], available: true },
    }),
  );
  await render(<App bridge={source} />);
  source.failWith = new BridgeError(
    "notification_store_unavailable",
    "private Rust disk path",
  );
  await click(button("通知，有未读消息"));
  expect(
    [...document.querySelectorAll("button")].some(
      (item) => item.textContent === "查看传输",
    ),
  ).toBe(false);
  await click(button("标为已读"));
  expect(document.body.textContent).toContain("已读状态未能保存");
  expect(document.body.textContent).not.toContain("private Rust disk path");
  await act(async () => setLanguage("en"));
  expect(document.body.textContent).toContain(
    "The read status could not be saved",
  );
  expect(document.body.textContent).toContain("中文 <合同>.pdf");
});
