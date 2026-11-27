import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import {
  DriveTreeBrowser,
  type DriveTreeLocation,
} from "@assemble/ui/drive-tree-browser";
import type {
  DriveFolder,
  DriveTreeListing,
  DriveTreeSource,
} from "@assemble/ui/lib/drive-tree";
import { isDriveTreeListing } from "@assemble/ui/lib/drive-tree";
import { driveTreeError } from "@assemble/ui/lib/drive-tree-i18n";
import { act } from "react";
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
import { renderApp } from "./test-support/render-app";

afterEach(async () => {
  await cleanup();
  setLanguage("zh-CN");
});
const folder: DriveFolder = {
  id: "folder-1",
  parentId: null,
  name: "资料<中文>",
  version: 1,
  createdAt: "2026-11-27T01:00:00.000Z",
  updatedAt: "2026-11-27T01:00:00.000Z",
};
function listing(current: DriveFolder | null = null): DriveTreeListing {
  return {
    current,
    ancestors: [],
    items: current ? [] : [{ kind: "folder", ...folder }],
    total: current ? 0 : 1,
    page: 1,
    pageSize: 10,
    usedBytes: 0,
    limitBytes: 1024,
  };
}
function dialog() {
  const result = document.querySelector<HTMLElement>('[role="dialog"]');
  if (!result) throw new Error("dialog missing");
  return result;
}

test("云盘根目录标题不重复，进入文件夹后标题与面包屑同步", async () => {
  const source = new FakeBridge(snapshot({ phase: "signedIn" }));
  source.driveTree = {
    list: async (query) => listing(query.parentId === "root" ? null : folder),
  };
  const view = await renderApp(source, "云盘");
  await settle();
  expect(document.title).toBe("云盘 · Assemble");
  await click(button(view.host, folder.name));
  await settle();
  expect(document.title).toBe(`${folder.name} · 云盘 · Assemble`);
  const heading = view.host.querySelector('[data-slot="breadcrumb-page"]');
  expect(heading?.textContent).toBe(folder.name);
  expect(document.activeElement).toBe(heading);
});

test("目录样本经两端契约守卫，拒绝非法版本与半份响应", () => {
  const sample = samples.find((sample) => sample.driveTree !== null)?.driveTree;
  expect(isDriveTreeListing(sample)).toBe(true);
  expect(isDriveTreeListing({ ...sample, pageSize: 101 })).toBe(false);
  expect(
    isDriveTreeListing({
      ...listing(),
      items: [{ kind: "folder", ...folder, version: 0 }],
    }),
  ).toBe(false);
  expect(
    isDriveTreeListing({
      ...listing(),
      current: { ...folder, createdAt: "invalid" },
    }),
  ).toBe(false);
});
test("创建使用弹窗，校验后只传名字父级和稳定请求键；服务拒绝保留草稿", async () => {
  const created: unknown[] = [];
  const source: DriveTreeSource = {
    list: async () => listing(),
    create: async (input) => {
      created.push(input);
      throw { code: "DRIVE_NAME_CONFLICT", message: "private Rust diagnostic" };
    },
  };
  const view = await render(
    <DriveTreeBrowser
      source={source}
      formatDate={(date) => date}
      formatSize={String}
    />,
  );
  await settle();
  await click(button(view.host, "新建文件夹"));
  expect(dialog().querySelector("label:not(.sr-only)")).toBeNull();
  await typeInto(dialog(), "../x");
  await click(button(dialog(), "创建"));
  expect(created).toHaveLength(0);
  expect(dialog().querySelector('[aria-invalid="true"]')).not.toBeNull();
  await typeInto(dialog(), "  资料<中文>  ");
  await click(button(dialog(), "创建"));
  expect(created).toHaveLength(1);
  expect(created[0]).toMatchObject({ parentId: "root", name: folder.name });
  expect(text(dialog())).toContain("同名文件夹");
  expect(text(dialog())).not.toContain("private Rust");
  await act(async () => setLanguage("en"));
  await settle();
  expect(text(dialog())).toContain("already exists");
  expect((dialog().querySelector("input") as HTMLInputElement).value).toContain(
    folder.name,
  );
  await click(button(dialog(), "Create"));
  expect(created[0]).toEqual(created[1]);
  await click(button(dialog(), "Cancel"));
  await settle();
  expect(document.querySelector('[role="dialog"]')).toBeNull();
});
test("父级通过面包屑返回，当前层搜索会恢复；只读目录不显示写入动作或空图标", async () => {
  const queries: unknown[] = [];
  let location: DriveTreeLocation | null = null;
  const source: DriveTreeSource = {
    list: async (query) => {
      queries.push(query);
      return {
        ...listing(query.parentId === "root" ? null : folder),
        page: query.page,
        pageSize: query.pageSize,
      };
    },
  };
  const onLocationChange = (value: DriveTreeLocation | null) => {
    location = value;
  };
  const view = await render(
    <DriveTreeBrowser
      source={source}
      defaultPageSize={20}
      onLocationChange={onLocationChange}
      formatDate={(date) => date}
      formatSize={String}
    />,
  );
  await settle();
  expect(queries.at(-1)).toMatchObject({ pageSize: 20, parentId: "root" });
  expect(button(view.host, "新建文件夹")).toBeUndefined();
  expect(view.host.querySelector("select")).toBeNull();
  await typeInto(view.host, "资料");
  await click(button(view.host, folder.name));
  await settle();
  expect(text(view.host)).toContain("暂无内容");
  expect(view.host.querySelector("tbody svg")).toBeNull();
  expect(location).toMatchObject({
    currentId: folder.id,
    current: folder.name,
  });
  const trail = location as DriveTreeLocation | null;
  await act(async () => trail?.ancestors[0]?.onNavigate?.());
  await settle();
  expect(queries.at(-1)).toMatchObject({ parentId: "root", q: "资料" });
  await click(button(view.host, "清空搜索"));
  await settle();
  expect(queries.at(-1)).toMatchObject({ q: "", page: 1 });
  expect(document.activeElement).toBe(
    view.host.querySelector('input[name="q"]'),
  );
});
test("只有根入口404回退旧列表，网络失败不隐藏错误；迟到结果不越过卸载边界", async () => {
  const source = new FakeBridge(snapshot());
  let resolve: ((value: DriveTreeListing) => void) | undefined;
  let aborted: AbortSignal | undefined;
  source.driveTree = {
    list: (_query, signal) => {
      aborted = signal;
      return new Promise((done) => {
        resolve = done;
      });
    },
  };
  const view = await render(
    <DriveWorkspace
      source={source}
      snapshot={snapshot({ phase: "signedIn" })}
      legacy={<p>legacy-list</p>}
      onLocationChange={() => {}}
    />,
  );
  await view.unmount();
  expect(aborted?.aborted).toBe(true);
  resolve?.(listing());
  await settle();
  expect(view.host.textContent).toBe("");
  source.driveTree = {
    list: async () => {
      throw { code: "NOT_FOUND" };
    },
  };
  const old = await render(
    <DriveWorkspace
      source={source}
      snapshot={snapshot({ phase: "signedIn" })}
      legacy={<p>legacy-list</p>}
      onLocationChange={() => {}}
    />,
  );
  await settle();
  expect(text(old.host)).toContain("legacy-list");
  await old.unmount();
  source.driveTree = {
    list: async () => {
      throw { code: "NETWORK" };
    },
  };
  const failed = await render(
    <DriveWorkspace
      source={source}
      snapshot={snapshot({ phase: "signedIn" })}
      legacy={<p>legacy-list</p>}
      onLocationChange={() => {}}
    />,
  );
  await settle();
  expect(text(failed.host)).toContain("暂时无法读取");
  expect(text(failed.host)).not.toContain("legacy-list");
  expect(driveTreeError({ message: "raw Rust diagnostic" })).toBe("failed");
});

test("移动需明确确认，选择目标不会提交；非空删除保留弹窗并给出双语原因", async () => {
  const moves: unknown[] = [];
  const queries: unknown[] = [];
  const source: DriveTreeSource = {
    list: async (query) => {
      queries.push(query);
      return listing(query.parentId === "root" ? null : folder);
    },
    move: async (input) => {
      moves.push(input);
      return { kind: "folder", ...folder };
    },
    remove: async () => {
      throw { code: "DRIVE_FOLDER_NOT_EMPTY", message: "SQL diagnostic" };
    },
  };
  const view = await render(
    <DriveTreeBrowser
      source={source}
      formatDate={(date) => date}
      formatSize={String}
    />,
  );
  await settle();
  async function action(label: string) {
    const menu = button(view.host, `${folder.name}的操作`);
    if (!menu) throw new Error("row actions missing");
    await act(async () => {
      menu.focus();
      menu.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
      );
    });
    await settle();
    const item = [
      ...document.querySelectorAll<HTMLElement>('[role="menuitem"]'),
    ].find((item) => item.textContent?.trim() === label);
    if (!item) throw new Error("menu action missing");
    await click(item);
  }
  await action("移动");
  await settle();
  expect(moves).toHaveLength(0);
  expect(queries.at(-1)).toMatchObject({ foldersOnly: true });
  const self = button(dialog(), folder.name);
  expect(self?.disabled).toBe(true);
  await click(button(dialog(), "移动到此处"));
  expect(moves).toHaveLength(1);
  expect(moves[0]).toMatchObject({
    kind: "folder",
    id: folder.id,
    parentId: "root",
    version: 1,
  });
  await settle();
  await action("删除");
  await settle();
  expect(document.activeElement).toBe(button(dialog(), "取消") ?? null);
  await click(button(dialog(), "确认删除"));
  expect(text(dialog())).toContain("仍有内容");
  expect(text(dialog())).not.toContain("SQL diagnostic");
  await act(async () => setLanguage("en"));
  expect(text(dialog())).toContain("still contains items");
});
