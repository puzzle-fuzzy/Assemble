// 按真实菜单进入对应页面；旧测试的“账户”简称指向账户中心。
import { App } from "@/App";
import type { Bridge } from "@/lib/bridge";
import { button, click, render, settle } from "@/test-support/dom";

export async function renderApp(source: Bridge, section?: string) {
  const view = await render(<App bridge={source} />);
  await settle();
  if (section)
    await click(button(view.host, section === "账户" ? "账户中心" : section));
  return view;
}
