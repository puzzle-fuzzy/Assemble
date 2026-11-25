// 按真实菜单进入对应页面；旧测试的“账户”简称指向账户中心。

import { act } from "react";
import { App } from "@/App";
import type { Bridge } from "@/lib/bridge";
import { button, click, render, settle } from "@/test-support/dom";

export async function renderApp(source: Bridge, section?: string) {
  const view = await render(<App bridge={source} />);
  await settle();
  if (section === "账户" || section === "账户中心")
    await navigateAccount(view.host);
  else if (section) await click(button(view.host, section));
  return view;
}

export async function openAccountMenu(host: HTMLElement) {
  const trigger = button(host, "账户菜单");
  if (!trigger) throw new Error("缺少侧栏账户菜单");
  await act(async () => {
    trigger.focus();
    trigger.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
    );
  });
  await settle();
  return trigger;
}

export async function navigateAccount(host: HTMLElement) {
  await openAccountMenu(host);
  const item = [
    ...document.body.querySelectorAll<HTMLElement>('[role="menuitem"]'),
  ].find((candidate) => candidate.textContent === "账户中心");
  await click(item);
}
