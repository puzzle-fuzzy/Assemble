import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { DataTable, type DataTableColumn } from "./data-table";

type Item = { id: string; name: string; note: string };
const data: Item[] = Array.from({ length: 12 }, (_, index) => ({
  id: String(index + 1),
  name: `条目${index + 1}`,
  note: `备注${index + 1}`,
}));
const columns: DataTableColumn<Item>[] = [
  { id: "name", label: "名称", cell: (item) => item.name, hideable: false },
  { id: "note", label: "备注", cell: (item) => item.note },
];
let host: HTMLDivElement;
let root: Root;
afterEach(async () => {
  await act(async () => {
    root?.unmount();
    setLanguage("zh-CN");
  });
  host?.remove();
});
async function mount(view: Parameters<Root["render"]>[0]) {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root.render(view));
}
function control(label: string) {
  const found = [...host.querySelectorAll<HTMLButtonElement>("button")].find(
    (item) =>
      (item.getAttribute("aria-label") ?? item.textContent?.trim()) === label,
  );
  if (!found) throw new Error(`Missing table control: ${label}`);
  return found;
}
async function click(element: HTMLElement) {
  await act(async () => element.click());
}
async function key(element: HTMLElement, value: string) {
  await act(async () => {
    element.focus();
    element.dispatchEvent(
      new KeyboardEvent("keydown", { key: value, bubbles: true }),
    );
  });
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 10));
  });
}
function option(role: string, label: string) {
  const found = [
    ...document.querySelectorAll<HTMLElement>(`[role="${role}"]`),
  ].find((item) => item.textContent?.trim() === label);
  if (!found) throw new Error(`Missing table menu item: ${label}`);
  return found;
}

test("本机表格只选当前页，翻页清除选择，列菜单确实隐藏对应数据", async () => {
  await mount(
    <DataTable
      label="测试表格"
      data={data}
      columns={columns}
      getRowLabel={(item) => item.name}
    />,
  );
  expect(host.querySelectorAll("tbody tr")).toHaveLength(10);
  await click(control("选择本页全部行"));
  expect(host.querySelectorAll('tbody tr[data-state="selected"]')).toHaveLength(
    10,
  );
  expect(host.textContent).toContain("已选 10 条，共 12 条");
  await click(control("下一页"));
  expect(host.querySelectorAll("tbody tr")).toHaveLength(2);
  expect(host.querySelectorAll('tbody tr[data-state="selected"]')).toHaveLength(
    0,
  );
  expect(host.textContent).toContain("第 2 页，共 2 页");
  await key(control("自定义列"), "ArrowDown");
  await click(option("menuitemcheckbox", "备注"));
  expect(
    [...host.querySelectorAll("th")].map((cell) => cell.textContent),
  ).not.toContain("备注");
  expect(host.querySelector("tbody")?.textContent).not.toContain("备注11");
  expect(host.querySelector("tbody")?.textContent).toContain("条目11");
});

test("服务端数据不再本机切页，条数和末页操作交回调用方，语言切换不改用户内容", async () => {
  const changes: Array<[number, number]> = [];
  await mount(
    <DataTable
      label="测试表格"
      data={data}
      columns={columns}
      getRowLabel={(item) => item.name}
      pagination={{
        page: 2,
        pageSize: 20,
        total: 75,
        onChange: (page, size) => changes.push([page, size]),
      }}
    />,
  );
  expect(host.querySelectorAll("tbody tr")).toHaveLength(12);
  expect(host.textContent).toContain("第 2 页，共 4 页");
  await click(control("最后一页"));
  expect(changes).toEqual([[4, 20]]);
  await key(control("每页条数"), "Enter");
  await key(option("option", "10"), "Enter");
  expect(changes).toEqual([
    [4, 20],
    [1, 10],
  ]);
  await act(async () => setLanguage("en"));
  expect(control("Customize columns")).toBeDefined();
  expect(control("Rows per page")).toBeDefined();
  expect(host.textContent).toContain("Page 2 of 4");
  expect(host.textContent).toContain("0 of 75 rows selected.");
  expect(host.textContent).toContain("条目1");
  expect(changes).toHaveLength(2);
});

test("结果集更替后不保留旧数据、勾选或旧行操作菜单", async () => {
  const selected: string[] = [];
  const view = (rows: Item[]) => (
    <DataTable
      label="测试表格"
      data={rows}
      columns={columns}
      getRowLabel={(item) => item.name}
      rowActions={(item) => [
        { label: "查看详情", onSelect: () => selected.push(item.id) },
      ]}
    />
  );
  await mount(view(data));
  await click(control("选择 条目1"));
  await key(control("条目1的操作"), "ArrowDown");
  await click(option("menuitem", "查看详情"));
  expect(selected).toEqual(["1"]);
  await key(control("条目1的操作"), "ArrowDown");
  await act(async () =>
    root.render(
      view([{ id: "another-account", name: "另一组数据", note: "新备注" }]),
    ),
  );
  expect(host.textContent).not.toContain("条目1");
  expect(document.querySelector('[role="menu"]')).toBeNull();
  expect(control("选择 另一组数据").getAttribute("aria-checked")).toBe("false");
  expect(host.querySelectorAll('tbody tr[data-state="selected"]')).toHaveLength(
    0,
  );
});

test("管理表格保留拖拽和多选列，文字操作、显示列和分页仍可用", async () => {
  const opened: string[] = [];
  await mount(
    <DataTable
      variant="management"
      label="管理列表"
      data={data}
      columns={columns}
      getRowLabel={(item) => item.name}
      rowActions={(item) => [
        { label: "详情", onSelect: () => opened.push(item.id) },
      ]}
    />,
  );
  expect(host.querySelectorAll('[role="checkbox"]')).toHaveLength(11);
  expect(host.querySelectorAll("[data-table-drag-handle]")).toHaveLength(10);
  expect(control("调整 条目1 的顺序").disabled).toBe(true);
  expect(
    [...host.querySelectorAll("th")].map((cell) => cell.textContent),
  ).toEqual(["调整行顺序", "", "名称", "备注", "操作"]);
  expect(host.textContent).not.toContain("已选");
  await click(control("详情：条目1"));
  expect(opened).toEqual(["1"]);
  await key(control("显示列"), "ArrowDown");
  await click(option("menuitemcheckbox", "备注"));
  expect(
    [...host.querySelectorAll("th")].map((cell) => cell.textContent),
  ).not.toContain("备注");
  await click(control("下一页"));
  expect(host.textContent).toContain("第 2 / 2 页");
  expect(control("详情：条目11")).toBeDefined();
  await act(async () => setLanguage("en"));
  expect(control("Columns")).toBeDefined();
  expect(host.textContent).toContain("Page 2 / 2");
});

test("禁用功能仍保留两列，部分不可选行不进入全选", async () => {
  const view = (selectable: boolean | ((item: Item) => boolean)) => (
    <DataTable
      variant="management"
      label="受限列表"
      data={data.slice(0, 3)}
      columns={columns}
      getRowLabel={(item) => item.name}
      selectable={selectable}
    />
  );
  await mount(view(false));
  expect(host.querySelector('th[data-table-column="drag"]')).not.toBeNull();
  expect(host.querySelector('th[data-table-column="select"]')).not.toBeNull();
  expect(host.querySelectorAll("tbody [data-table-drag-handle]")).toHaveLength(
    3,
  );
  for (const checkbox of host.querySelectorAll<HTMLButtonElement>(
    '[role="checkbox"]',
  ))
    expect(checkbox.disabled).toBe(true);
  await act(async () => root.render(view((item) => item.id !== "2")));
  await click(control("选择本页全部行"));
  expect(host.querySelectorAll('tbody tr[data-state="selected"]')).toHaveLength(
    2,
  );
  expect(control("选择 条目2").disabled).toBe(true);
  expect(control("选择本页全部行").getAttribute("aria-checked")).toBe("true");
});

function OrderedTable() {
  const [rows, setRows] = useState(data.slice(0, 3));
  return (
    <DataTable
      variant="management"
      label="可排序列表"
      data={rows}
      columns={columns}
      getRowLabel={(item) => item.name}
      onReorder={setRows}
      paginate={false}
    />
  );
}
function rowOrder() {
  return [
    ...host.querySelectorAll<HTMLTableRowElement>("tbody tr[data-table-row]"),
  ].map((row) => row.dataset.tableRow);
}
async function altArrow(
  element: HTMLElement,
  direction: "ArrowUp" | "ArrowDown",
) {
  await act(async () => {
    element.focus();
    element.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: direction,
        altKey: true,
        bubbles: true,
      }),
    );
  });
}
test("键盘排序保留行节点、焦点和选择，首末边界不越界，排序不会提交外围表单", async () => {
  let submitted = 0;
  await mount(
    <form
      onSubmit={(event) => {
        event.preventDefault();
        submitted += 1;
      }}
    >
      <OrderedTable />
    </form>,
  );
  await click(control("选择 条目1"));
  const first = host.querySelector('tr[data-table-row="1"]');
  const handle = control("调整 条目1 的顺序");
  expect(handle.disabled).toBe(false);
  await altArrow(handle, "ArrowDown");
  expect(rowOrder()).toEqual(["2", "1", "3"]);
  expect(host.querySelector('tr[data-table-row="1"]')).toBe(first);
  expect(document.activeElement).toBe(handle);
  expect(control("选择 条目1").getAttribute("aria-checked")).toBe("true");
  await altArrow(handle, "ArrowDown");
  await altArrow(handle, "ArrowDown");
  expect(rowOrder()).toEqual(["2", "3", "1"]);
  await click(control("显示列"));
  expect(submitted).toBe(0);
  expect(host.querySelector('nav[aria-label="表格分页"]')).toBeNull();
  await act(async () => setLanguage("en"));
  expect(control("Reorder 条目1")).toBe(handle);
});

function prepareGeometry() {
  const body = host.querySelector("tbody");
  if (!body) throw new Error("missing table body");
  body.getBoundingClientRect = () => new DOMRect(0, 0, 600, 144);
  for (const row of body.querySelectorAll<HTMLTableRowElement>("tr")) {
    row.getBoundingClientRect = () =>
      new DOMRect(0, [...body.children].indexOf(row) * 48, 600, 48);
    const handle = row.querySelector<HTMLButtonElement>(
      "[data-table-drag-handle]",
    );
    if (!handle) continue;
    const captured = new Set<number>();
    handle.setPointerCapture = (id) => {
      captured.add(id);
    };
    handle.hasPointerCapture = (id) => captured.has(id);
    handle.releasePointerCapture = (id) => {
      captured.delete(id);
    };
  }
}
async function pointer(element: HTMLElement, type: string, y: number, id = 1) {
  await act(async () =>
    element.dispatchEvent(
      new PointerEvent(type, {
        pointerId: id,
        isPrimary: true,
        clientY: y,
        clientX: 10,
        button: 0,
        bubbles: true,
      }),
    ),
  );
}
test("指针拖拽在释放时调整数据顺序，第二个指针不能抢走拖拽，Escape 可取消", async () => {
  await mount(<OrderedTable />);
  prepareGeometry();
  const handle = control("调整 条目1 的顺序");
  await pointer(handle, "pointerdown", 24);
  await pointer(handle, "pointermove", 130, 2);
  await pointer(handle, "pointerup", 130, 2);
  expect(rowOrder()).toEqual(["1", "2", "3"]);
  await pointer(handle, "pointermove", 130);
  expect(rowOrder()).toEqual(["1", "2", "3"]);
  await pointer(handle, "pointerup", 130);
  expect(rowOrder()).toEqual(["2", "3", "1"]);
  expect(
    document.documentElement.classList.contains("table-row-dragging"),
  ).toBe(false);
  await pointer(handle, "pointerdown", 120);
  await pointer(handle, "pointermove", 10);
  await act(async () =>
    document.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Escape",
        bubbles: true,
        cancelable: true,
      }),
    ),
  );
  await pointer(handle, "pointerup", 10);
  expect(rowOrder()).toEqual(["2", "3", "1"]);
  expect(document.activeElement).toBe(handle);
});

test("新结果即使沿用相同行标识，也会取消旧指针拖拽", async () => {
  const moves: Item[][] = [];
  const view = (rows: Item[]) => (
    <DataTable
      label="查询结果"
      data={rows}
      columns={columns}
      getRowLabel={(item) => item.name}
      onReorder={(items) => moves.push(items)}
      paginate={false}
    />
  );
  await mount(view(data.slice(0, 3)));
  prepareGeometry();
  const handle = control("调整 条目1 的顺序");
  await pointer(handle, "pointerdown", 24);
  await pointer(handle, "pointermove", 130);
  expect(host.querySelector("[data-table-dragging]")).not.toBeNull();
  await act(async () =>
    root.render(
      view(
        data.slice(0, 3).map((item) => ({ ...item, name: `新${item.name}` })),
      ),
    ),
  );
  expect(host.querySelector("[data-table-dragging]")).toBeNull();
  expect(host.querySelector("[data-table-drop]")).toBeNull();
  expect(
    document.documentElement.classList.contains("table-row-dragging"),
  ).toBe(false);
  await pointer(handle, "pointerup", 130);
  expect(moves).toEqual([]);
  expect(rowOrder()).toEqual(["1", "2", "3"]);
});
