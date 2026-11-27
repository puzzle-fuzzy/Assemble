import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { isTransferView } from "@/lib/guard";
import { transferStatus, verdict } from "@/lib/transfer-text";
import type { TransferView } from "@/lib/types";
import { button, cleanup, click, render } from "@/test-support/dom";
import { FakeBridge, snapshot } from "@/test-support/fakes";
import { UploadControls } from "@/UploadControls";

const transfer: TransferView = {
  id: "upload-one",
  direction: "out",
  target: "drive",
  peer: "云盘",
  name: "中文 <原始文件>.txt",
  size: 100,
  phase: "inProgress",
  progress: 0.5,
  failure: null,
  savedAs: null,
  retryable: false,
  resumable: true,
};
afterEach(async () => {
  await cleanup();
  setLanguage("zh-CN");
});
test("恢复下载使用相同控制，重新定位只接受任务标识并保留文件名", async () => {
  const source = new FakeBridge(snapshot({ phase: "signedIn" }));
  setLanguage("en");
  const incoming = {
    ...transfer,
    direction: "in" as const,
    phase: "paused" as const,
  };
  const view = await render(
    <UploadControls source={source} transfer={incoming} onError={() => {}} />,
  );
  await click(button(view.host, `Resume transfer of ${transfer.name}`));
  expect(source.calls).toEqual(["resumeTransfer:upload-one"]);
  await cleanup();
  setLanguage("zh-CN");
  const missing = {
    ...transfer,
    phase: "paused" as const,
    failure: "fileUnreadable" as const,
  };
  const next = await render(
    <UploadControls source={source} transfer={missing} onError={() => {}} />,
  );
  await click(button(next.host, `重新选择原文件 ${transfer.name}`));
  expect(source.calls[1]).toBe("relocateTransfer:upload-one");
  expect(transferStatus(missing)).toContain("原文件");
  setLanguage("en");
  expect(transferStatus(missing)).toContain("Original file");
  expect(
    transferStatus({
      ...incoming,
      phase: "failed",
      failure: "destinationChanged",
    }),
  ).toContain("receive folder");
  setLanguage("zh-CN");
  expect(
    transferStatus({
      ...incoming,
      phase: "failed",
      failure: "destinationChanged",
    }),
  ).toContain("接收文件夹");
});
test("暂停和继续只传任务标识，按钮名称包含原样文件名", async () => {
  setLanguage("zh-CN");
  const source = new FakeBridge(snapshot({ phase: "signedIn" }));
  const view = await render(
    <UploadControls source={source} transfer={transfer} onError={() => {}} />,
  );
  await click(button(view.host, `暂停传输 ${transfer.name}`));
  expect(source.calls).toEqual(["pauseTransfer:upload-one"]);
  await cleanup();
  setLanguage("en");
  const paused = { ...transfer, phase: "paused" as const };
  const next = await render(
    <UploadControls
      source={source}
      transfer={paused}
      onError={() => {}}
      withCancel
    />,
  );
  await click(button(next.host, `Resume transfer of ${transfer.name}`));
  await click(button(next.host, `Cancel transfer of ${transfer.name}`));
  expect(source.calls).toEqual([
    "pauseTransfer:upload-one",
    "resumeTransfer:upload-one",
    "cancelTransfer:upload-one",
  ]);
  expect(next.host.querySelector("script") === null).toBe(true);
});
test("旧端和终态不出现续传操作，暂停保留百分比且不算已送达", async () => {
  const source = new FakeBridge(snapshot());
  for (const value of [
    { ...transfer, resumable: undefined },
    { ...transfer, phase: "complete" as const },
  ]) {
    const view = await render(
      <UploadControls source={source} transfer={value} onError={() => {}} />,
    );
    expect(view.host.querySelector("button") === null).toBe(true);
    await cleanup();
  }
  const paused = { ...transfer, phase: "paused" as const };
  expect(isTransferView(paused)).toBe(true);
  expect(isTransferView({ ...paused, resumable: "yes" })).toBe(false);
  setLanguage("zh-CN");
  expect(transferStatus(paused)).toBe("已暂停 · 50%");
  setLanguage("en");
  expect(transferStatus(paused)).toBe("Paused · 50%");
  expect(verdict([paused])).toBe("sending");
  expect(
    transferStatus({
      ...paused,
      phase: "failed",
      failure: "destinationChanged",
    }),
  ).toContain("destination folder changed");
});
