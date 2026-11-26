import { expect, test } from "bun:test";
import { untilPageSettles } from "./dom";

test("加载中的页面即使没有文字变化，也要等待忙碌状态结束", async () => {
  const host = document.createElement("div");
  const loading = document.createElement("div");
  loading.setAttribute("aria-busy", "true");
  host.append(loading);
  let ready = false;
  const waiting = untilPageSettles(host).then(() => {
    ready = true;
  });
  try {
    // 给等待器多轮观察机会；它独占 act，避免并行轮询造成测试环境互相干扰。
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(host.textContent).toBe("");
    expect(ready).toBe(false);
  } finally {
    loading.remove();
    await waiting;
  }
  expect(ready).toBe(true);
});
