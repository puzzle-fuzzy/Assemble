import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { BridgeError } from "@/lib/bridge";
import { cleanup, click, render, text } from "@/test-support/dom";
import { FakeBridge, snapshot } from "@/test-support/fakes";
import { renderApp } from "@/test-support/render-app";
import { AssistantDownloadsSetting } from "./AssistantDownloadsSetting";

afterEach(async () => {
  await cleanup();
  setLanguage("zh-CN");
});

test("本机设置可关闭助手下载，按 Rust 新快照显示开关状态", async () => {
  const source = new FakeBridge(snapshot());
  const view = await renderApp(source);
  const settings = view.host.querySelector<HTMLElement>(
    'button[data-sidebar="menu-button"][aria-label="本机设置"]',
  );
  const fallback = [...view.host.querySelectorAll<HTMLElement>("button")].find(
    (item) => item.textContent === "本机设置",
  );
  await click(settings ?? fallback);
  const toggle = view.host.querySelector<HTMLElement>(
    '[role="switch"][aria-label="允许助手在这台电脑下载"]',
  );
  expect(toggle?.getAttribute("aria-checked")).toBe("true");
  await click(toggle ?? undefined);
  expect(source.calls).toContain("downloadSetAssistantAllowed:false");
  expect(toggle?.getAttribute("aria-checked")).toBe("false");
});

test("助手开关使用英语，保存失败不改变值或暴露原始诊断", async () => {
  setLanguage("en");
  const source = new FakeBridge(snapshot());
  source.downloadSetAssistantAllowed = async () => {
    throw new BridgeError("DOWNLOAD_STORAGE", "Rust: C:/private/secret.json");
  };
  const view = await render(
    <AssistantDownloadsSetting source={source} enabled />,
  );
  const toggle = view.host.querySelector<HTMLElement>('[role="switch"]');
  expect(toggle?.getAttribute("aria-label")).toBe(
    "Allow assistant downloads on this computer",
  );
  await click(toggle ?? undefined);
  expect(toggle?.getAttribute("aria-checked")).toBe("true");
  expect(text(view.host)).toContain("Could not save the file");
  expect(text(view.host)).not.toContain("private");
  expect(text(view.host)).not.toContain("Rust:");
});
