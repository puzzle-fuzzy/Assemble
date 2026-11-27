import { expect, test } from "bun:test";
import type { SequencedInput } from "./remote";
import { RemoteInputQueue, remoteKey, remotePoint } from "./remote-input";

test("坐标按实际画面映射，留白不产生输入，键盘使用稳定字符", () => {
  const bounds = { left: 20, top: 10, width: 200, height: 100 };
  expect(remotePoint(20, 10, bounds)).toEqual({ type: "move", x: 0, y: 0 });
  expect(remotePoint(220, 110, bounds)).toEqual({
    type: "move",
    x: 65535,
    y: 65535,
  });
  expect(remotePoint(10, 20, bounds)).toBeNull();
  expect(remoteKey("A", "KeyA")).toBe("a");
  expect(remoteKey("Dead", "Quote")).toBeNull();
});
test("输入只有一个在途请求，移动合并且按键顺序不会丢失", async () => {
  const sent: SequencedInput[][] = [];
  let finish: (() => void) | undefined;
  const queue = new RemoteInputQueue(
    async (batch) => {
      sent.push(batch);
      if (sent.length === 1)
        await new Promise<void>((resolve) => {
          finish = resolve;
        });
    },
    () => {
      throw new Error("unexpected failure");
    },
  );
  queue.push({ type: "key", key: "Control", pressed: true });
  for (let x = 0; x < 100; x++) queue.push({ type: "move", x, y: 0 });
  queue.push({ type: "button", button: "left", pressed: true });
  queue.push({ type: "releaseAll" });
  expect(sent).toHaveLength(1);
  finish?.();
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(sent[1]?.map((e) => e.input)).toEqual([
    { type: "move", x: 99, y: 0 },
    { type: "button", button: "left", pressed: true },
    { type: "releaseAll" },
  ]);
  expect(sent.flat().map((e) => e.sequence)).toEqual([1, 2, 3, 4]);
});
test("投递失败后停止，不盲目重放文字", async () => {
  let failed = 0;
  let calls = 0;
  const queue = new RemoteInputQueue(
    async () => {
      calls++;
      throw new Error("network");
    },
    () => failed++,
  );
  queue.push({ type: "text", text: "中文" });
  await new Promise((resolve) => setTimeout(resolve, 0));
  queue.push({ type: "text", text: "不重放" });
  expect(failed).toBe(1);
  expect(calls).toBe(1);
});
