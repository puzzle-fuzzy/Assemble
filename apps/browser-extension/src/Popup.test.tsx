import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import {
  button,
  cleanup,
  click,
  render,
  settle,
  text,
} from "../../desktop/src/test-support/dom";
import { errorText } from "./i18n";
import en from "./locales/en.json";
import zh from "./locales/zh-CN.json";
import { Popup } from "./Popup";
import type { PopupReply, PopupRequest, SourceList } from "./protocol";

afterEach(async () => {
  await cleanup();
  setLanguage("zh-CN");
});
test("点击下载显示加入结果，重复点击不会重复提交，标题保持原样", async () => {
  const list: SourceList = {
    capturing: false,
    items: [
      {
        id: "one",
        kind: "hls",
        title: "中文 <视频>",
        host: "cdn.example.test",
        bytes: null,
        submitted: false,
      },
    ],
  };
  let calls = 0;
  let release = () => {};
  const send = async (request: PopupRequest): Promise<PopupReply> => {
    if (request.type === "download") {
      calls++;
      await new Promise<void>((resolve) => {
        release = resolve;
      });
      list.items = list.items.map((item) => ({ ...item, submitted: true }));
    }
    return { ok: true, list: { ...list, items: [...list.items] } };
  };
  const view = await render(
    <Popup send={send} requestCapture={async () => true} />,
  );
  await settle();
  expect(text(view.host)).toContain("中文 <视频>");
  const download = button(view.host, "下载 中文 <视频>");
  await act(async () => {
    download?.click();
    download?.click();
  });
  expect(calls).toBe(1);
  expect(text(view.host)).toContain("加入中…");
  await act(async () => release());
  await settle();
  expect(text(view.host)).toContain("已加入");
});
test("网络权限由明确点击触发，中英文错误不回显诊断", async () => {
  let permissionCalls = 0;
  const send = async (request: PopupRequest): Promise<PopupReply> =>
    request.type === "scan"
      ? { ok: true, list: { items: [], capturing: false } }
      : { ok: false, code: "private raw diagnostic" };
  const view = await render(
    <Popup
      send={send}
      requestCapture={async () => {
        permissionCalls++;
        return false;
      }}
    />,
  );
  await settle();
  expect(permissionCalls).toBe(0);
  await click(button(view.host, "网络捕获"));
  expect(permissionCalls).toBe(1);
  expect(text(view.host)).toContain("未开启网络捕获");
  await act(async () => setLanguage("en"));
  expect(text(view.host)).toContain("Network capture was not enabled");
  expect(errorText("private raw diagnostic")).toBe(
    "The operation did not complete. Try again.",
  );
  expect(Object.keys(en).sort()).toEqual(Object.keys(zh).sort());
  expect(Object.keys(en.error).sort()).toEqual(Object.keys(zh.error).sort());
});
