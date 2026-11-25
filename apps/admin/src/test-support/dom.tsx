import { act, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";

const mounted: { host: HTMLElement; root: Root }[] = [];

export async function render(ui: ReactElement) {
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  mounted.push({ host, root });
  await act(async () => root.render(ui));
  await settle();
  return host;
}

export async function cleanup() {
  for (const { host, root } of mounted.splice(0)) {
    await act(async () => root.unmount());
    host.remove();
  }
}

export async function settle() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 5));
  });
}

export function button(host: HTMLElement, name: string) {
  const found = [...host.querySelectorAll<HTMLButtonElement>("button")].find(
    (item) =>
      (item.getAttribute("aria-label") ?? item.textContent?.trim()) === name,
  );
  if (!found) throw new Error(`找不到按钮：${name}`);
  return found;
}

export async function click(element: HTMLElement) {
  await act(async () => element.click());
  await settle();
}

export async function navigate(host: HTMLElement, view: string) {
  const link = host.querySelector<HTMLAnchorElement>(
    `nav[aria-label="管理功能"] a[href="#${view}"]`,
  );
  if (!link) throw new Error(`找不到管理菜单：${view}`);
  await click(link);
}

export async function search(host: HTMLElement, value: string) {
  const input = host.querySelector<HTMLInputElement>('input[type="search"]');
  if (!input) throw new Error("找不到搜索输入框");
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set?.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await click(button(host, "搜索"));
}
