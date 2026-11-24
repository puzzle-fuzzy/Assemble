// 界面测试共用的 DOM 工具：把组件渲染进真实 DOM，等待它稳定，再按文字找元素。
// 主窗口与发送面板的测试都用它。

import { act, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";

export interface Rendered {
  host: HTMLElement;
  unmount: () => Promise<void>;
}

/** 这次测试里挂载过的根。每个测试结束后统一卸载。 */
const mounted: { root: Root; host: HTMLElement }[] = [];

export async function render(ui: ReactElement): Promise<Rendered> {
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  mounted.push({ root, host });
  await act(async () => {
    root.render(ui);
  });
  return {
    host,
    unmount: async () => {
      await act(async () => {
        root.unmount();
      });
    },
  };
}

/** 卸载这次测试挂载的所有组件。测试结束后调用。 */
export async function cleanup() {
  for (const { root, host } of mounted.splice(0)) {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
}

/** 等待已排队的回调与零延时定时器跑完。 */
export async function settle() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 5));
  });
}

/** 等真实的时间过去：给那些刻意停一会儿的动作用。 */
export async function wait(milliseconds: number) {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, milliseconds));
  });
}

export function text(host: HTMLElement) {
  return host.textContent ?? "";
}

function named(
  host: HTMLElement,
  name: string,
  selector: string,
): HTMLButtonElement[] {
  return [...host.querySelectorAll<HTMLButtonElement>(selector)].filter(
    (candidate) => candidate.textContent?.trim() === name,
  );
}

export function button(
  host: HTMLElement,
  name: string,
): HTMLButtonElement | undefined {
  return named(host, name, "button")[0];
}

export function buttons(host: HTMLElement, name: string) {
  return named(host, name, "button");
}

export function textField(host: HTMLElement): HTMLInputElement {
  const found = host.querySelector<HTMLInputElement>(
    "input:not([type=checkbox])",
  );
  if (!found) throw new Error("没有找到输入框");
  return found;
}

/** 在输入框里打字：走浏览器自己的 value 设置器，React 才会看到变化。 */
export async function typeInto(host: HTMLElement, value: string) {
  await act(async () => {
    const input = textField(host);
    const setter = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set;
    setter?.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

export function launchSwitch(host: HTMLElement): HTMLButtonElement {
  const found = host.querySelector<HTMLButtonElement>('[role="switch"]');
  if (!found) throw new Error("没有找到开机启动开关");
  return found;
}

/** 点击但不等待命令返回：用来观察"进行中"的状态。 */
export async function press(element: HTMLElement | undefined) {
  if (!element) throw new Error("要点击的元素不存在");
  await act(async () => {
    element.click();
  });
}

/** 点击并等命令返回、界面稳定。 */
export async function click(element: HTMLElement | undefined) {
  await press(element);
  await settle();
}
