import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import {
  DriveTrashBrowser,
  TrashConfirm,
} from "@assemble/ui/drive-trash-browser";
import {
  isTrashListing,
  isTrashRestorePreview,
  type TrashBatch,
  type TrashRestorePreview,
  type TrashSource,
} from "@assemble/ui/lib/drive-trash";
import type { DriveEntry } from "@assemble/ui/lib/drive-tree";
import { act } from "react";
import { toast } from "sonner";
import samples from "../contract/snapshots.json";
import { DriveWorkspace } from "./DriveWorkspace";
import {
  button,
  cleanup,
  click,
  render,
  settle,
  text,
  typeInto,
} from "./test-support/dom";
import { FakeBridge, snapshot } from "./test-support/fakes";

afterEach(async () => {
  toast.dismiss();
  await cleanup();
  await act(async () => setLanguage("zh-CN"));
});
const batch: TrashBatch = {
  id: "batch-1",
  rootId: "folder-1",
  kind: "folder",
  name: "资料<中文>",
  originalLocation: ["项目"],
  version: 1,
  deletedAt: "2026-11-27T08:00:00.000Z",
  fileCount: 2,
  folderCount: 1,
  size: 75,
};
const entry: DriveEntry = {
  id: batch.rootId,
  kind: "folder",
  parentId: null,
  name: batch.name,
  version: 1,
  createdAt: batch.deletedAt,
  updatedAt: batch.deletedAt,
};
const plan: TrashRestorePreview = {
  batchId: batch.id,
  id: batch.rootId,
  kind: "folder",
  name: "资料<中文>（恢复）",
  parentId: null,
  parentLocation: [],
  reason: "parentUnavailable",
  renamed: true,
  version: 1,
  revision: "a".repeat(64),
};
const listing = { items: [batch], total: 1, page: 1, pageSize: 20 };
function source(): TrashSource {
  return {
    list: async () => listing,
    detail: async () => {
      throw { code: "NOT_FOUND" };
    },
  };
}
function dialog() {
  const result = document.querySelector<HTMLElement>('[role="dialog"]');
  if (!result) throw new Error("dialog missing");
  return result;
}

test("回收样本和响应守卫拒绝坏版本、缺少字段及非法修订", () => {
  const sample = samples.find((item) => item.driveTrash !== null)?.driveTrash;
  expect(isTrashListing(sample)).toBe(true);
  expect(
    isTrashListing({ ...listing, items: [{ ...batch, version: 0 }] }),
  ).toBe(false);
  expect(isTrashListing({ ...listing, pageSize: 101 })).toBe(false);
  expect(isTrashRestorePreview(plan)).toBe(true);
  expect(isTrashRestorePreview({ ...plan, revision: "server secret" })).toBe(
    false,
  );
});
test("移入必须先读影响摘要再确认，失败保留弹窗和稳定请求键，错误双语且不泄漏诊断", async () => {
  const calls: unknown[] = [];
  let saved = 0;
  const service = {
    ...source(),
    preview: async () => ({
      target: { kind: entry.kind, id: entry.id, version: 1 },
      name: entry.name,
      fileCount: 2,
      folderCount: 1,
      size: 75,
      revision: "a".repeat(64),
    }),
    move: async (input: unknown) => {
      calls.push(input);
      throw { code: "DRIVE_TRASH_CHANGED", message: "private SQL diagnostic" };
    },
  };
  await render(
    <TrashConfirm
      source={service}
      operation={{ mode: "move", entry }}
      formatSize={String}
      onClose={() => {}}
      onSaved={() => saved++}
    />,
  );
  await settle();
  expect(calls).toHaveLength(0);
  expect(text(dialog())).toContain("2 个文件");
  expect(document.activeElement === button(dialog(), "取消")).toBe(true);
  await click(button(dialog(), "移入回收站"));
  expect(calls).toHaveLength(1);
  expect(saved).toBe(0);
  expect(text(dialog())).toContain("重新核对");
  expect(text(dialog())).not.toContain("private SQL");
  await act(async () => setLanguage("en"));
  await settle();
  expect(text(dialog())).toContain("destination changed");
  expect(text(dialog())).toContain("2 files · 1 folder · 75");
  expect(text(dialog())).toContain(entry.name);
  await click(button(dialog(), "Move to trash"));
  expect(calls[0]).toEqual(calls[1]);
});
test("恢复展示真实位置和重名后的名称，永久删除是独立确认，取消不写入", async () => {
  const calls: unknown[] = [];
  const service = {
    ...source(),
    restorePreview: async () => plan,
    restore: async (id: string, input: unknown) => {
      calls.push({ id, input });
      return plan;
    },
    purge: async () => {
      calls.push("purge");
      return { id: batch.id, logicalBytesReleased: 75, cleanupPending: true };
    },
  };
  let restored: unknown = null;
  const view = await render(
    <TrashConfirm
      source={service}
      operation={{ mode: "restore", batch }}
      formatSize={String}
      onClose={() => {}}
      onSaved={(value) => {
        restored = value;
      }}
    />,
  );
  await settle();
  expect(calls).toHaveLength(0);
  expect(text(dialog())).toContain(plan.name);
  expect(text(dialog())).toContain("原文件夹已不可用");
  await click(button(dialog(), "恢复"));
  expect(restored).toEqual(plan);
  expect(calls).toEqual([
    { id: batch.id, input: { version: 1, revision: plan.revision } },
  ]);
  await view.unmount();
  let closed = false;
  await render(
    <TrashConfirm
      source={service}
      operation={{ mode: "purge", batch }}
      formatSize={String}
      onClose={() => {
        closed = true;
      }}
      onSaved={() => {}}
    />,
  );
  await settle();
  expect(document.activeElement === button(dialog(), "取消")).toBe(true);
  await click(button(dialog(), "取消"));
  expect(closed).toBe(true);
  expect(calls).toHaveLength(1);
  await click(button(dialog(), "永久删除"));
  expect(calls.at(-1)).toBe("purge");
});
test("只读回收站分页搜索和父级跳转，无写操作、原生下拉或空图标", async () => {
  const queries: unknown[] = [];
  let trail:
    | import("@assemble/ui/drive-tree-browser").DriveTreeLocation
    | null = null;
  const service = {
    ...source(),
    list: async (q: string, page: number, pageSize: number) => {
      queries.push({ q, page, pageSize });
      return {
        ...listing,
        items: q ? [] : [batch],
        total: q ? 0 : 1,
        page,
        pageSize,
      };
    },
    detail: async () => ({
      batch,
      current: {
        id: batch.rootId,
        parentId: null,
        kind: "folder" as const,
        name: batch.name,
        version: 2,
        size: null,
        createdAt: batch.deletedAt,
      },
      ancestors: [],
      items: [],
      total: 0,
      page: 1,
      pageSize: 20,
    }),
  };
  const view = await render(
    <DriveTrashBrowser
      source={service}
      defaultPageSize={20}
      formatSize={String}
      formatDate={String}
      onFiles={() => {}}
      onLocationChange={(value) => {
        trail = value;
      }}
    />,
  );
  await settle();
  expect(queries.at(-1)).toMatchObject({ pageSize: 20 });
  expect(button(view.host, "恢复")).toBeUndefined();
  expect(view.host.querySelector("select")).toBeNull();
  await click(button(view.host, batch.name));
  await settle();
  expect(trail).toMatchObject({ current: batch.name });
  expect(view.host.querySelector("tbody svg")).toBeNull();
  const path = trail as
    | import("@assemble/ui/drive-tree-browser").DriveTreeLocation
    | null;
  await act(async () => path?.ancestors[1]?.onNavigate?.());
  await settle();
  await typeInto(view.host, "不存在");
  await settle();
  expect(text(view.host)).toContain("暂无内容");
  await click(button(view.host, "清空搜索"));
  await settle();
  expect((document.activeElement as HTMLElement).tagName).toBe("INPUT");
});
test("旧服务不支持回收站时保留读取但不提供永久删除退路，卸载后忽略迟到写入", async () => {
  const bridge = new FakeBridge(snapshot({ phase: "signedIn" }));
  bridge.driveTree = {
    list: async () => ({
      current: null,
      ancestors: [],
      items: [entry],
      total: 1,
      page: 1,
      pageSize: 10,
      usedBytes: 0,
      limitBytes: 100,
    }),
    remove: async () => {
      throw new Error("permanent fallback must not run");
    },
  };
  const view = await render(
    <DriveWorkspace
      source={bridge}
      snapshot={snapshot({ phase: "signedIn" })}
      legacy={<p>legacy</p>}
      onLocationChange={() => {}}
    />,
  );
  await settle();
  expect(text(view.host)).toContain("服务端尚不支持回收站");
  expect(button(view.host, `${entry.name}的操作`) === undefined).toBe(true);
  await view.unmount();
  let resolve: ((value: TrashBatch) => void) | undefined;
  let saved = false;
  const service = {
    ...source(),
    preview: async () => ({
      target: { kind: entry.kind, id: entry.id, version: 1 },
      name: entry.name,
      fileCount: 2,
      folderCount: 1,
      size: 75,
      revision: "a".repeat(64),
    }),
    move: () =>
      new Promise<TrashBatch>((done) => {
        resolve = done;
      }),
  };
  const pending = await render(
    <TrashConfirm
      source={service}
      operation={{ mode: "move", entry }}
      formatSize={String}
      onClose={() => {}}
      onSaved={() => {
        saved = true;
      }}
    />,
  );
  await settle();
  await click(button(dialog(), "移入回收站"));
  await pending.unmount();
  await act(async () => resolve?.(batch));
  await settle();
  expect(saved).toBe(false);
});

test("回收记录减少后退到有效页，保留搜索与条数，不把旧空页留给用户", async () => {
  let total = 11;
  const queries: { q: string; page: number; size: number }[] = [];
  const service = {
    ...source(),
    list: async (q: string, page: number, size: number) => {
      queries.push({ q, page, size });
      return {
        items: page === 1 || total === 11 ? [batch] : [],
        total,
        page,
        pageSize: size,
      };
    },
  };
  const view = await render(
    <DriveTrashBrowser
      source={service}
      formatSize={String}
      formatDate={String}
      onFiles={() => {}}
    />,
  );
  await settle();
  await typeInto(view.host, "资料");
  await settle();
  await click(button(view.host, "下一页"));
  await settle();
  expect(queries.at(-1)?.page).toBe(2);
  total = 10;
  await click(button(view.host, "刷新"));
  await settle();
  expect(queries.at(-1)).toEqual({ q: "资料", page: 1, size: 10 });
  expect(view.host.querySelector<HTMLInputElement>("input")?.value).toBe(
    "资料",
  );
});
