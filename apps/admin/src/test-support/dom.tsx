import { act, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { adminText } from "../lib/i18n";

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
    `nav[aria-label="${adminText("navigation.label")}"] a[href="#${view}"]`,
  );
  if (!link) throw new Error(`找不到管理菜单：${view}`);
  await click(link);
  await untilPageSettles(host);
}

/// 等到条件成立为止；等不到就如实失败，不让后面的断言去猜。
///
/// 点完导航、刷新或搜索，这一页的数据是稍后才到的。点完立刻去断言，就是和加载赛跑：
/// 2026-11-26 持续集成的两栏同时输过一次（页面还停在"正在加载"）。
export async function waitFor(ready: () => boolean, what: string) {
  const deadline = Date.now() + 5_000;
  while (!ready()) {
    if (Date.now() > deadline) throw new Error(`等不到：${what}`);
    await settle();
  }
}

/// 等到页面里出现这段文字。
export async function waitForText(host: HTMLElement, text: string) {
  await waitFor(() => (host.textContent ?? "").includes(text), text);
}

/// 加载指示前 200ms 没有文字，必须同时核对忙碌状态，不能把空白的懒加载页面判为就绪。
export async function untilPageSettles(host: HTMLElement) {
  let previous: string | null = null;
  let calm = 0;
  await waitFor(() => {
    const current = host.textContent ?? "";
    const busy = host.querySelector('[aria-busy="true"]') !== null;
    calm = !busy && current === previous ? calm + 1 : 0;
    previous = current;
    return calm >= 3;
  }, "页面加载完成并稳定");
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
  await click(button(host, adminText("common.search")));
}
