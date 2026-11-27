import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import invalidTimes from "../contract/invalid-times.json";
import fixture from "../contract/releases.json";
import { App } from "./App";
import { MAX_MANIFEST_BYTES, parseManifest } from "./lib/catalog";
import { loadReleases } from "./lib/load";

const originalFetch = globalThis.fetch;
let root: Root;
let host: HTMLDivElement;
let response: () => Response | Promise<Response>;
const requests: RequestInit[] = [];
beforeEach(() => {
  setLanguage("zh-CN");
  document.body.replaceChildren();
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  requests.length = 0;
  response = () => Response.json(fixture);
  globalThis.fetch = mock((_url, init) => {
    requests.push(init ?? {});
    return Promise.resolve(response());
  }) as unknown as typeof fetch;
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  globalThis.fetch = originalFetch;
});
async function mount() {
  await act(async () => root.render(<App />));
  await ready();
}
async function ready() {
  for (
    let index = 0;
    index < 100 && host.querySelector('main[aria-busy="true"]');
    index++
  )
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
}
function button(label: string) {
  const found = [
    ...document.querySelectorAll<HTMLButtonElement>("button"),
  ].find(
    (item) =>
      (item.getAttribute("aria-label") ?? item.textContent?.trim()) === label,
  );
  if (!found) throw new Error(`Missing button ${label}`);
  return found;
}
async function click(element: HTMLElement) {
  await act(async () => {
    element.focus();
    element.click();
  });
}
test("仅实际产物有固定下载地址，未发行平台无链接，校验弹窗保留焦点", async () => {
  await mount();
  expect(host.querySelectorAll("a[download]")).toHaveLength(2);
  expect(host.querySelectorAll("section")).toHaveLength(4);
  for (const platform of ["ios", "android"]) {
    const section = host.querySelector(
      `section[aria-labelledby="platform-${platform}"]`,
    );
    expect(section?.textContent).toContain("未发布");
    expect(section?.querySelector("a")).toBeNull();
  }
  expect(host.querySelector("a[download]")?.getAttribute("href")).toBe(
    `/v1/releases/files/${fixture.artifacts[0].sha256}`,
  );
  expect(requests[0].credentials).toBe("omit");
  expect(requests[0].redirect).toBe("error");
  const trigger = button("查看 Windows x64 0.1.0.abc1234 的校验信息");
  await click(trigger);
  expect(document.querySelector('[role="dialog"]')?.textContent).toContain(
    fixture.artifacts[0].sha256,
  );
  await click(button("关闭"));
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 10));
  });
  expect(document.querySelector('[role="dialog"]')).toBeNull();
  expect(document.activeElement).toBe(trigger);
});
test("读取失败不回显响应正文，真实刷新操作能够恢复", async () => {
  response = () => new Response("private diagnostic", { status: 503 });
  await mount();
  expect(host.textContent).toContain("暂时无法读取发布信息");
  expect(host.textContent).not.toContain("private diagnostic");
  response = () => Response.json(fixture);
  await click(button("重试"));
  await ready();
  expect(host.querySelectorAll("a[download]")).toHaveLength(2);
  expect(requests).toHaveLength(2);
});
test("切换语言更新状态和操作名称，不重复请求或改写真实版本", async () => {
  await mount();
  await act(async () => setLanguage("en"));
  expect(host.textContent).toContain("Download Assemble");
  expect(host.textContent).toContain("Not released");
  expect(host.querySelector("a[download]")?.getAttribute("aria-label")).toBe(
    "Download Windows x64 0.1.0.abc1234",
  );
  expect(document.title).toBe("Download Assemble");
  expect(requests).toHaveLength(1);
});
test("清单中的任意 URL、非法摘要、重复版本或超量内容都不能变为下载入口", async () => {
  expect(() =>
    parseManifest({ ...fixture, url: "https://evil.test" }),
  ).toThrow();
  expect(() =>
    parseManifest({
      ...fixture,
      artifacts: [{ ...fixture.artifacts[0], sha256: "../../secret" }],
    }),
  ).toThrow();
  expect(() =>
    parseManifest({
      ...fixture,
      artifacts: [fixture.artifacts[0], fixture.artifacts[0]],
    }),
  ).toThrow();
  response = () =>
    Response.json({
      ...fixture,
      artifacts: [
        { ...fixture.artifacts[0], downloadUrl: "https://evil.test" },
      ],
    });
  await mount();
  expect(host.querySelector("a[download]")).toBeNull();
  expect(host.textContent).toContain("暂时无法读取发布信息");
  response = () => new Response(" ".repeat(MAX_MANIFEST_BYTES + 1));
  await expect(loadReleases(new AbortController().signal)).rejects.toThrow(
    "RELEASES_UNAVAILABLE",
  );
});
test("空清单如实显示四个平台未发行，不用演示包填充", async () => {
  response = () => Response.json({ schemaVersion: 1, artifacts: [] });
  await mount();
  expect(host.querySelectorAll("a[download]")).toHaveLength(0);
  expect(host.textContent?.match(/未发布/g)).toHaveLength(4);
});
test("页面退出取消读取，迟到响应不能恢复页面内容", async () => {
  let resolve: ((value: Response) => void) | undefined;
  response = () =>
    new Promise((done) => {
      resolve = done;
    });
  await act(async () => root.render(<App />));
  expect(
    host.querySelector('[role="status"]')?.getAttribute("aria-label"),
  ).toBe("正在读取发布信息");
  await act(async () => root.render(null));
  expect(requests[0].signal?.aborted).toBe(true);
  await act(async () => resolve?.(Response.json(fixture)));
  expect(host.textContent).toBe("");
});

test("发布时间拒绝自动滚日和非规范偏移，与 Rust 共用非法日期样本", () => {
  for (const publishedAt of invalidTimes) {
    expect(() =>
      parseManifest({
        ...fixture,
        artifacts: [{ ...fixture.artifacts[0], publishedAt }],
      }),
    ).toThrow("INVALID_RELEASE_MANIFEST");
  }
  expect(
    parseManifest({
      ...fixture,
      artifacts: [
        { ...fixture.artifacts[0], publishedAt: "2024-02-29T23:59:59.123Z" },
      ],
    }).artifacts,
  ).toHaveLength(1);
});
