import { afterEach, expect, mock, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import { AdminSystemNotifications } from "./components/system-notifications";
import { button, cleanup, click, render, settle } from "./test-support/dom";

const originalFetch = globalThis.fetch;
afterEach(async () => {
  await cleanup();
  globalThis.fetch = originalFetch;
  setLanguage("zh-CN");
});

test("打开的通知弹窗自动接收后台同步结果，不需要刷新按钮", async () => {
  let arrived = false;
  globalThis.fetch = mock(async () =>
    Response.json({ items: arrived ? [item] : [], available: true }),
  ) as unknown as typeof fetch;
  const host = await render(
    <AdminSystemNotifications
      session={session}
      onAccessFailure={() => {}}
      onStorage={() => {}}
    />,
  );
  await click(button(host, "通知"));
  const dialog = document.querySelector('[role="dialog"]');
  expect(dialog?.textContent).toContain("暂无系统通知");
  arrived = true;
  await act(async () => window.dispatchEvent(new Event("focus")));
  await settle();
  expect(document.querySelector('[role="dialog"]')).toBe(dialog);
  expect(dialog?.textContent).toContain("服务器磁盘空间不足");
  expect(dialog?.textContent).not.toContain("刷新");
});
const item = {
  id: "storage-warning",
  revision: 3,
  content: { type: "storageDisk", availableBytes: 1024 },
  level: "error",
  createdAt: "2026-11-26T01:00:00.000Z",
  updatedAt: "2026-11-26T01:00:00.000Z",
  resolvedAt: null,
  readAt: null,
};
const session = {
  account: {
    id: "admin",
    displayName: "管理员",
    email: "admin@example.test",
    avatarUrl: null,
  },
  csrfToken: "synthetic-csrf",
};
test("管理端通过同源具名接口读写通知，只在服务确认后改变已读，不回显错误正文", async () => {
  const calls: { path: string; init?: RequestInit }[] = [];
  let fail = true;
  globalThis.fetch = mock(
    async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      calls.push({ path, init });
      if (path.endsWith("/read"))
        return fail
          ? Response.json(
              { diagnostic: "private Rust message" },
              { status: 503 },
            )
          : Response.json({
              items: [{ ...item, readAt: "2026-11-26T01:01:00.000Z" }],
              available: true,
            });
      return Response.json({ items: [item], available: true });
    },
  ) as unknown as typeof fetch;
  const onStorage = mock(() => {});
  const host = await render(
    <AdminSystemNotifications
      session={session}
      onAccessFailure={() => {}}
      onStorage={onStorage}
    />,
  );
  await click(button(host, "通知，有未读消息"));
  expect(document.querySelector('[data-slot="dialog-content"]')).toBeTruthy();
  expect(document.querySelector('[role="dialog"]')?.textContent).not.toContain(
    "刷新",
  );
  await click(button(document.body, "标为已读"));
  expect(document.body.textContent).toContain("已读状态未能保存");
  expect(document.body.textContent).not.toContain("private Rust message");
  expect(calls.at(-1)?.init?.headers).toEqual({
    Accept: "application/json",
    "X-CSRF-Token": "synthetic-csrf",
    "Content-Type": "application/json",
  });
  expect(calls.at(-1)?.init?.credentials).toBe("same-origin");
  expect(JSON.parse(String(calls.at(-1)?.init?.body))).toEqual({
    items: [{ id: "storage-warning", revision: 3 }],
  });
  fail = false;
  await click(button(document.body, "标为已读"));
  expect(document.body.textContent).not.toContain("已读状态未能保存");
  await click(button(document.body, "查看存储"));
  expect(onStorage).toHaveBeenCalledTimes(1);
  await settle();
});
