// 按真实入口进入对应区域，账户浮层通过 Portal 挂在 body 中。
import { App } from "@/App";
import type { Bridge } from "@/lib/bridge";
import { button, click, render, settle } from "@/test-support/dom";

export async function renderApp(source: Bridge, section?: string) {
  const view = await render(<App bridge={source} />);
  await settle();
  if (section) await click(button(view.host, section));
  return { ...view, host: section === "账户" ? document.body : view.host };
}
