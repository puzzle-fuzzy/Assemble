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
  const visible = host.cloneNode(true) as HTMLElement;
  for (const hidden of visible.querySelectorAll(
    "[hidden], [aria-hidden='true'], .sr-only, style",
  ))
    hidden.remove();
  return visible.textContent ?? "";
}

/** 标题是这个名字的那张卡片：只在它里面找东西，免得和别的卡片里同名的按钮弄混。 */
export function card(host: HTMLElement, title: string): HTMLElement {
  const found = [
    ...host.querySelectorAll<HTMLElement>('[data-slot="card"]'),
  ].find(
    (candidate) =>
      !candidate.closest("[hidden]") &&
      candidate
        .querySelector('[data-slot="card-title"]')
        ?.textContent?.trim() === title,
  );
  if (!found) throw new Error(`没有找到「${title}」卡片`);
  return found;
}

/** 名单里标题是这个名字的那几行（日程卡片里，通知和安排里可能各有一行）。 */
export function rows(host: HTMLElement, title: string): HTMLElement[] {
  return [...host.querySelectorAll<HTMLElement>("li")].filter(
    (line) => line.querySelector("h3")?.textContent?.trim() === title,
  );
}

/** 同上，取头一个。 */
export function row(host: HTMLElement, title: string): HTMLElement {
  const [first] = rows(host, title);
  if (!first) throw new Error(`没有找到「${title}」这一行`);
  return first;
}

/** 小标题是这个名字的那份名单。 */
export function listUnder(host: HTMLElement, heading: string): HTMLElement {
  const title = [...host.querySelectorAll("h2")].find(
    (candidate) => candidate.textContent?.trim() === heading,
  );
  const list = title?.parentElement?.querySelector("ul");
  if (!list) throw new Error(`没有找到「${heading}」下面的名单`);
  return list;
}

function named(
  host: HTMLElement,
  name: string,
  selector: string,
): HTMLButtonElement[] {
  return [...host.querySelectorAll<HTMLButtonElement>(selector)].filter(
    (candidate) =>
      !candidate.closest("[hidden]") &&
      (candidate.getAttribute("aria-label") === name ||
        candidate.textContent?.trim() === name),
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
  const found = [
    ...host.querySelectorAll<HTMLInputElement>("input:not([type=checkbox])"),
  ].find((candidate) => !candidate.closest("[hidden]"));
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

export type Field = HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement;

/** 按标签文字找一个表单控件：标签用编号连着它的控件。 */
export function field(host: HTMLElement, label: string): Field {
  const tag = [...host.querySelectorAll("label")].find(
    (candidate) => candidate.textContent?.trim() === label,
  );
  const control = tag?.control;
  if (
    !control ||
    !(
      control instanceof HTMLInputElement ||
      control instanceof HTMLTextAreaElement ||
      control instanceof HTMLSelectElement
    )
  ) {
    throw new Error(`没有找到「${label}」`);
  }
  return control;
}

/** 往表单控件里填一个值：走浏览器自己的 value 设置器，React 才会看到变化。 */
export async function fill(host: HTMLElement, label: string, value: string) {
  await act(async () => {
    const control = field(host, label);
    const prototype =
      control instanceof HTMLSelectElement
        ? HTMLSelectElement.prototype
        : control instanceof HTMLTextAreaElement
          ? HTMLTextAreaElement.prototype
          : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, "value")?.set?.call(
      control,
      value,
    );
    // 文本与时间控件改值时发 input；下拉框发 change。
    control.dispatchEvent(
      new Event(control instanceof HTMLSelectElement ? "change" : "input", {
        bubbles: true,
      }),
    );
  });
}

export function launchSwitch(host: HTMLElement): HTMLButtonElement {
  const found = host.querySelector<HTMLButtonElement>('[role="switch"]');
  if (!found) throw new Error("没有找到开机启动开关");
  return found;
}

/** 按提示文字找一个开关。 */
export function switchNamed(
  host: HTMLElement,
  label: string,
): HTMLButtonElement {
  const found = host.querySelector<HTMLButtonElement>(
    `[role="switch"][aria-label="${label}"]`,
  );
  if (!found) throw new Error(`没有找到「${label}」开关`);
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
