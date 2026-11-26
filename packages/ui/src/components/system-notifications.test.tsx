import { afterEach, expect, mock, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import {
  isSystemNotifications,
  type SystemNotification,
} from "../lib/system-notifications";
import { SystemNotificationCenter } from "./system-notifications";

let root: Root | undefined;
let host: HTMLElement | undefined;
afterEach(async () => {
  await act(async () => {
    root?.unmount();
    setLanguage("zh-CN");
  });
  host?.remove();
});
const notification: SystemNotification = {
  id: "system-one",
  revision: 1,
  content: {
    type: "storageBudget",
    usedBytes: 80 * 1024,
    budgetBytes: 100 * 1024,
    warningPercent: 80,
  },
  level: "warning",
  createdAt: "2026-11-26T01:00:00.000Z",
  updatedAt: "2026-11-26T01:00:00.000Z",
  readAt: null,
  resolvedAt: null,
};
async function render(
  items: SystemNotification[],
  onRead = mock(() => {}),
  onNavigate = mock(() => {}),
) {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () =>
    root?.render(
      <SystemNotificationCenter
        items={items}
        onRead={onRead}
        onNavigate={onNavigate}
      />,
    ),
  );
  return { onRead, onNavigate };
}
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
async function click(element: HTMLElement) {
  await act(async () => element.click());
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 10));
  });
}

test("通知只在用户打开时出现，支持取消回焦点、已读版本与中英文变量", async () => {
  const actions = await render([notification]);
  const trigger = button("通知，有未读消息");
  expect(document.querySelector('[role="dialog"]')).toBeNull();
  await click(trigger);
  expect(document.querySelector('[data-slot="dialog-content"]')).toBeTruthy();
  expect(document.querySelector('[data-slot="sheet-content"]')).toBeNull();
  expect(document.querySelector('[role="dialog"]')?.textContent).not.toContain(
    "刷新",
  );
  expect(document.body.textContent).toContain("80 KiB / 100 KiB · 预警线 80%");
  expect(
    document.querySelector('[role="dialog"]')?.getAttribute("aria-labelledby"),
  ).toBeTruthy();
  await click(button("标为已读"));
  expect(actions.onRead).toHaveBeenCalledWith([
    { id: "system-one", revision: 1 },
  ]);
  await act(async () => setLanguage("en"));
  expect(document.body.textContent).toContain("Warning at 80%");
  expect(document.querySelector('[role="dialog"]')?.textContent).not.toContain(
    "Refresh",
  );
  await click(button("Close"));
  expect(document.activeElement).toBe(trigger);
});

test("后台失败只展示原样文件名，不把文件名当HTML；前往任务会提交已读并导航", async () => {
  const task: SystemNotification = {
    ...notification,
    content: {
      type: "downloadFailed",
      taskId: "task",
      name: "中文 <img src=x>.mp4",
      localDiskFull: true,
    },
    level: "error",
  };
  const actions = await render([task]);
  await click(button("通知，有未读消息"));
  const dialog = document.querySelector('[role="dialog"]');
  expect(dialog?.textContent).toContain("本机存储空间不足");
  expect(dialog?.textContent).toContain("中文 <img src=x>.mp4");
  expect(dialog?.querySelector("img")).toBeNull();
  await click(button("查看下载"));
  expect(actions.onNavigate).toHaveBeenCalledWith(task);
  expect(actions.onRead).toHaveBeenCalledWith([{ id: task.id, revision: 1 }]);
});

test("空通知只显示文字，未知消息类型与坏日期不能通过桥接守卫", async () => {
  await render([]);
  await click(button("通知"));
  const dialog = document.querySelector('[role="dialog"]');
  expect(dialog?.textContent).toContain("暂无系统通知");
  expect(dialog?.querySelector("ul")).toBeNull();
  expect(
    isSystemNotifications({ items: [notification], available: true }),
  ).toBe(true);
  for (const item of [
    { ...notification, content: { type: "diagnostic", message: "private" } },
    { ...notification, updatedAt: "not-a-date" },
    { ...notification, revision: 0 },
  ])
    expect(isSystemNotifications({ items: [item], available: true })).toBe(
      false,
    );
});
