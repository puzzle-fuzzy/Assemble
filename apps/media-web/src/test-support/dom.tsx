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
  if (!found) throw new Error(`BUTTON_NOT_FOUND: ${name}`);
  return found;
}

export async function click(element: HTMLElement) {
  await act(async () => element.click());
  await settle();
}

export async function change(host: HTMLElement, id: string, value: string) {
  const control = host.querySelector<HTMLInputElement | HTMLSelectElement>(
    `#${id}`,
  );
  if (!control) throw new Error(`CONTROL_NOT_FOUND: ${id}`);
  await act(async () => {
    const prototype =
      control.tagName === "SELECT"
        ? HTMLSelectElement.prototype
        : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, "value")?.set?.call(
      control,
      value,
    );
    control.dispatchEvent(
      new Event(control.tagName === "SELECT" ? "change" : "input", {
        bubbles: true,
      }),
    );
  });
}
