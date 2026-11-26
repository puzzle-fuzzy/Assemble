import { afterEach, expect, spyOn, test } from "bun:test";
import { button, cleanup, click, render } from "../test-support/dom";
import { ReaderBoundary } from "./reader-boundary";

afterEach(cleanup);

function FailedChunk(): never {
  throw new Error("CHUNK_LOAD_FAILED");
}

test("详情分块加载失败保留外部导航并提供明确重试入口", async () => {
  const errors = spyOn(console, "error").mockImplementation(() => {});
  let retries = 0;
  try {
    const host = await render(
      <>
        <ReaderBoundary
          retry={() => {
            retries++;
          }}
        >
          <FailedChunk />
        </ReaderBoundary>
        <a href="/movies/">片库导航</a>
      </>,
    );
    expect(host.querySelector('[role="alert"]')).not.toBeNull();
    expect(host.textContent).toContain("暂时无法载入");
    expect(host.textContent).not.toContain("CHUNK_LOAD_FAILED");
    expect(host.querySelector('a[href="/movies/"]')).not.toBeNull();
    await click(button(host, "重试"));
    expect(retries).toBe(1);
  } finally {
    errors.mockRestore();
  }
});
