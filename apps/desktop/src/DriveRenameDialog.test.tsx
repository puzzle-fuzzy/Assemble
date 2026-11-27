import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import { cleanDriveFileName, DriveRenameAction } from "@/DriveRenameDialog";
import { BridgeError } from "@/lib/bridge";
import {
  button,
  cleanup,
  click,
  render,
  settle,
  text,
  textField,
  typeInto,
} from "@/test-support/dom";
import { FakeBridge, snapshot } from "@/test-support/fakes";

afterEach(async () => {
  await cleanup();
  setLanguage("zh-CN");
});

const file = {
  id: "drive-file-1",
  name: "合同<原件>.pdf",
  size: 123,
  createdAt: "2026-11-27T01:00:00Z",
};

function dialog() {
  const element = document.querySelector<HTMLElement>('[role="dialog"]');
  if (!element) throw new Error("rename dialog missing");
  return element;
}

test("云盘重命名使用弹窗，仅把标识和完整新名字交给具名命令", async () => {
  const source = new FakeBridge(snapshot({ phase: "signedIn" }));
  const view = await render(<DriveRenameAction source={source} file={file} />);
  await click(button(view.host, "重命名"));
  expect(textField(dialog()).value).toBe(file.name);
  expect(document.activeElement).toBe(textField(dialog()));
  expect(dialog().querySelector('[aria-invalid="true"]')).toBeNull();
  await typeInto(dialog(), "  共享<新合同>🌙.pdf  ");
  await click(button(dialog(), "保存"));
  expect(source.calls).toEqual(["driveRename:drive-file-1:共享<新合同>🌙.pdf"]);
  expect(document.querySelector('[role="dialog"]')).toBeNull();
});

test("空名字与路径被拦下；错误只在提交后出现，改正后可重试", async () => {
  const source = new FakeBridge(snapshot({ phase: "signedIn" }));
  const view = await render(<DriveRenameAction source={source} file={file} />);
  await click(button(view.host, "重命名"));
  for (const invalid of [" ", "../合同.pdf", "子目录\\合同.pdf", ".."]) {
    await typeInto(dialog(), invalid);
    expect(dialog().querySelector('[aria-invalid="true"]')).toBeNull();
    await click(button(dialog(), "保存"));
    const input = textField(dialog());
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(input.getAttribute("aria-describedby")).toBeTruthy();
    expect(document.activeElement).toBe(input);
  }
  expect(source.calls).toHaveLength(0);
  await typeInto(dialog(), "有效名.pdf");
  await click(button(dialog(), "保存"));
  expect(source.calls).toEqual(["driveRename:drive-file-1:有效名.pdf"]);
});

test("服务端拒绝保留草稿，英文只显示本地文案；取消后再打开恢复当前名字", async () => {
  const source = new FakeBridge(snapshot({ phase: "signedIn" }));
  const view = await render(<DriveRenameAction source={source} file={file} />);
  await click(button(view.host, "重命名"));
  await typeInto(dialog(), "保留的草稿<中文>.pdf");
  source.failWith = new BridgeError("NOT_FOUND", "Rust secret diagnostic");
  await click(button(dialog(), "保存"));
  expect(text(dialog())).toContain("文件已不存在");
  expect(textField(dialog()).value).toBe("保留的草稿<中文>.pdf");
  await act(async () => setLanguage("en"));
  expect(text(dialog())).toContain("This file no longer exists");
  expect(text(dialog())).not.toContain("Rust secret diagnostic");
  expect(textField(dialog()).value).toBe("保留的草稿<中文>.pdf");
  await click(button(dialog(), "Cancel"));
  await click(button(view.host, "Rename"));
  expect(textField(dialog()).value).toBe(file.name);
});

test("保存中禁止重复提交和取消，不改变文件名内容", async () => {
  const source = new FakeBridge(snapshot({ phase: "signedIn" }));
  let finish = () => {};
  const pending = new Promise<ReturnType<typeof snapshot>>((resolve) => {
    finish = () => resolve(source.current);
  });
  let calls = 0;
  const view = await render(
    <DriveRenameAction
      source={{
        driveRename: () => {
          calls += 1;
          return pending;
        },
      }}
      file={file}
    />,
  );
  await click(button(view.host, "重命名"));
  await click(button(dialog(), "保存"));
  expect(button(dialog(), "正在保存")?.disabled).toBe(true);
  expect(button(dialog(), "取消")?.disabled).toBe(true);
  const form = dialog().querySelector("form");
  await act(async () => {
    form?.dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
  });
  expect(calls).toBe(1);
  await act(async () => finish());
  await settle();
  expect(document.querySelector('[role="dialog"]')).toBeNull();
});

test("文件名按协议的 UTF-16 上限校验，同名和中文不作改写", () => {
  expect(cleanDriveFileName(file.name)).toBe(file.name);
  expect(cleanDriveFileName("名".repeat(255))).toHaveLength(255);
  expect(cleanDriveFileName("名".repeat(256))).toBeNull();
  expect(cleanDriveFileName("🌙".repeat(128))).toBeNull();
  expect(cleanDriveFileName("控制\u0085字符.pdf")).toBeNull();
});
