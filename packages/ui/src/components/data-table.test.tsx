import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
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
