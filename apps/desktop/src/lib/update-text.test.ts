import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import type { UpdateView } from "@/lib/types";
import { updateStatus } from "@/lib/update-text";

beforeEach(() => {
  setLanguage("zh-CN");
});
afterEach(() => {
  setLanguage("zh-CN");
});

function view(
  state: UpdateView["state"],
  extra: Partial<UpdateView> = {},
): UpdateView {
  return {
    state,
    version: null,
    received: null,
    total: null,
    checkedAt: null,
    error: null,
    ...extra,
  };
}

describe("自动更新的说法", () => {
  test("每一步都有一句话，不漏出内部状态名", () => {
    const lines = [
      updateStatus(view("unsupported")),
      updateStatus(view("idle")),
      updateStatus(view("checking")),
      updateStatus(view("upToDate")),
      updateStatus(
        view("downloading", {
          version: "0.1.432",
          received: 524288,
          total: 8388608,
        }),
      ),
      updateStatus(view("ready", { version: "0.1.432" })),
      updateStatus(view("failed", { error: "UPDATE_UNREACHABLE" })),
    ];
    expect(lines).toEqual([
      "这个构建不自动更新。",
      "还没有查过有没有新版本。",
      "正在查有没有新版本…",
      "已是最新版本。",
      "正在下载新版本 0.1.432：512 KB / 8.0 MB",
      "新版本 0.1.432 已经准备好：空闲时自动换上，也可以现在就换。",
      "更新没有成（UPDATE_UNREACHABLE），稍后会再试。",
    ]);
    for (const line of lines) expect(line).not.toContain("upToDate");
  });

  test("总量还不知道的时候只说已经下了多少", () => {
    expect(
      updateStatus(view("downloading", { version: "0.1.5", received: 100 })),
    ).toBe("正在下载新版本 0.1.5：100 B");
  });

  test("英文的说法也在", () => {
    setLanguage("en");
    expect(updateStatus(view("ready", { version: "0.1.432" }))).toBe(
      "Version 0.1.432 is ready. It is applied when Assemble is idle, or you can apply it now.",
    );
  });
});
