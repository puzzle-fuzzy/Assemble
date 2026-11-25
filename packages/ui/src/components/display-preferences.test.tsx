import { afterEach, expect, test } from "bun:test";
import {
  getDisplayPreferences,
  setLanguage,
  setTheme,
} from "@assemble/display";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { DisplayPreferencesMenu } from "./display-preferences";

let root: Root | undefined;
let host: HTMLDivElement | undefined;
afterEach(async () => {
  await act(async () => {
    root?.unmount();
    setLanguage("zh-CN");
    setTheme("system");
  });
  host?.remove();
});

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

function item(text: string, role = "menuitem") {
  const element = [
    ...document.querySelectorAll<HTMLElement>(`[role="${role}"]`),
  ].find((entry) => entry.textContent === text);
  if (!element) throw new Error(`missing menu option: ${text}`);
  return element;
}

test("共享 shadcn 菜单可用键盘切换语言与外观，并保留单选状态", async () => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => {
    root?.render(<DisplayPreferencesMenu />);
  });
  const trigger = host.querySelector("button");
  if (!trigger) throw new Error("missing preference trigger");
  expect(trigger.getAttribute("aria-label")).toBe("语言与外观");
  await key(trigger, "Enter");
  await key(item("语言"), "ArrowRight");
  expect(item("简体中文", "menuitemradio").getAttribute("aria-checked")).toBe(
    "true",
  );
  await key(item("English", "menuitemradio"), "Enter");
  expect(getDisplayPreferences().language).toBe("en");
  expect(document.documentElement.lang).toBe("en");
  expect(trigger.getAttribute("aria-label")).toBe("Language and appearance");
  await key(trigger, "Enter");
  await key(item("Appearance"), "ArrowRight");
  await key(item("Dark", "menuitemradio"), "Enter");
  expect(getDisplayPreferences().theme).toBe("dark");
  expect(document.documentElement.classList.contains("dark")).toBe(true);
  await key(trigger, "Enter");
  await key(item("Appearance"), "ArrowRight");
  expect(item("Dark", "menuitemradio").getAttribute("aria-checked")).toBe(
    "true",
  );
  await key(item("System", "menuitemradio"), "Enter");
  expect(getDisplayPreferences().theme).toBe("system");
});
