import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { PageBreadcrumbs } from "./page-breadcrumbs";

let host: HTMLDivElement;
let root: Root;
afterEach(async () => {
  await act(async () => {
    root?.unmount();
    setLanguage("zh-CN");
  });
  host?.remove();
});
function Example() {
  const [detail, setDetail] = useState(true);
  return (
    <PageBreadcrumbs
      ancestors={
        detail
          ? [
              {
                id: "parent",
                label: "片库",
                href: "#media",
                onNavigate: () => setDetail(false),
              },
            ]
          : []
      }
      current={detail ? "很长的作品名称，完整保留而不省略" : "片库"}
    />
  );
}

test("父级使用真实链接，修饰点击保留原生行为，普通跳转聚焦新当前位置", async () => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root.render(<Example />));
  const parent = host.querySelector("a");
  if (!parent) throw new Error("MISSING_ANCESTOR");
  expect(parent.getAttribute("href")).toBe("#media");
  expect(host.querySelectorAll('[aria-current="page"]')).toHaveLength(1);
  expect(
    host.querySelector('[data-slot="breadcrumb-page"]')?.getAttribute("title"),
  ).toBe("很长的作品名称，完整保留而不省略");
  expect(
    host
      .querySelector('[data-slot="breadcrumb-separator"]')
      ?.getAttribute("aria-hidden"),
  ).toBe("true");
  const modified = new MouseEvent("click", {
    bubbles: true,
    cancelable: true,
    ctrlKey: true,
  });
  await act(async () => parent.dispatchEvent(modified));
  expect(modified.defaultPrevented).toBe(false);
  expect(host.querySelector('[aria-current="page"]')?.textContent).toContain(
    "很长的作品名称",
  );
  await act(async () => parent.click());
  await act(
    async () =>
      new Promise<void>((resolve) =>
        window.requestAnimationFrame(() => resolve()),
      ),
  );
  const current = host.querySelector('[aria-current="page"]');
  expect(current?.textContent).toBe("片库");
  expect(document.activeElement).toBe(current);
  expect(host.querySelector("a")).toBeNull();
});
