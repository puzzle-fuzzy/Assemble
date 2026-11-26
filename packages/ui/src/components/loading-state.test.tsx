import { afterEach, beforeEach, expect, jest, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { LoadingState } from "./loading-state";

let host: HTMLDivElement;
let root: Root;
beforeEach(() => {
  jest.useFakeTimers();
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => {
    root.unmount();
    setLanguage("zh-CN");
  });
  host.remove();
  jest.useRealTimers();
});

test("短请求保持空白加载区，200ms 后才显示指示，不显示骨架块", async () => {
  await act(async () => root.render(<LoadingState />));
  expect(host.querySelector('[role="status"]')?.getAttribute("aria-busy")).toBe(
    "true",
  );
  expect(host.querySelector('[data-slot="spinner"]')).toBeNull();
  await act(async () => jest.advanceTimersByTime(199));
  expect(host.querySelector('[data-slot="spinner"]')).toBeNull();
  await act(async () => jest.advanceTimersByTime(1));
  expect(host.querySelector('[data-slot="spinner"]')).not.toBeNull();
  expect(host.querySelector('[data-slot="skeleton"]')).toBeNull();
});

test("请求先结束后不出现迟到的指示，也不移动输入焦点", async () => {
  await act(async () =>
    root.render(
      <>
        <input aria-label="搜索" />
        <LoadingState />
      </>,
    ),
  );
  const input = host.querySelector("input");
  input?.focus();
  await act(async () => jest.advanceTimersByTime(100));
  await act(async () => root.render(<input aria-label="搜索" />));
  await act(async () => jest.advanceTimersByTime(1000));
  expect(host.querySelector('[data-slot="spinner"]')).toBeNull();
  expect(document.activeElement).toBe(input);
});

test("正在显示的加载状态切换语言，不重置指示或丢失具体名称", async () => {
  await act(async () => root.render(<LoadingState />));
  await act(async () => jest.advanceTimersByTime(200));
  const spinner = host.querySelector('[data-slot="spinner"]');
  expect(
    host.querySelector('[role="status"]')?.getAttribute("aria-label"),
  ).toBe("正在加载");
  await act(async () => setLanguage("en"));
  expect(
    host.querySelector('[role="status"]')?.getAttribute("aria-label"),
  ).toBe("Loading");
  expect(host.querySelector('[data-slot="spinner"]')).toBe(spinner);
  await act(async () =>
    root.render(<LoadingState label="Loading schedules" />),
  );
  expect(
    host.querySelector('[role="status"]')?.getAttribute("aria-label"),
  ).toBe("Loading schedules");
});
