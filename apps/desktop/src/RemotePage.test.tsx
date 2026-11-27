import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act, StrictMode } from "react";
import type { RemoteBridge, RemoteSession, SequencedInput } from "@/lib/remote";
import { RemoteBanner, RemotePage } from "@/RemotePage";
import { button, cleanup, click, render, settle } from "@/test-support/dom";

const session: RemoteSession = {
  id: "r1",
  controllerId: "controller",
  controllerName: "Windows",
  hostId: "host",
  hostName: "Mac",
  phase: "pending",
  createdAt: "2026-11-27T00:00:00Z",
};
function fake() {
  const calls: string[] = [];
  const inputs: SequencedInput[][] = [];
  const source: RemoteBridge = {
    request: async () => session,
    approve: async () => {
      calls.push("approve");
      return { ...session, phase: "active" };
    },
    end: async () => {
      calls.push("end");
    },
    poll: async () => ({ session, frame: null, inputs: [] }),
    inputs: async (_id, events) => {
      inputs.push(events);
    },
    fullscreen: async () => {},
  };
  return { source, calls, inputs };
}
afterEach(async () => {
  await cleanup();
  await setLanguage("zh-CN");
});

test("左右修饰键共享按下状态，普通按键保留重复，失焦释放全部", async () => {
  const f = fake();
  const active: RemoteSession = { ...session, phase: "active" };
  f.source.poll = async () => ({
    session: active,
    frame: {
      sequence: 1,
      displayId: 1,
      width: 16,
      height: 16,
      jpeg: "AA==",
      displays: [{ id: 1, name: "Display", width: 16, height: 16 }],
    },
    inputs: [],
  });
  const view = await render(
    <RemotePage
      source={f.source}
      view={{ sessions: [active] }}
      devices={[]}
      ownId="controller"
      onSendFiles={() => {}}
    />,
  );
  await settle();
  const surface = view.host.querySelector<HTMLElement>('[role="application"]');
  if (!surface) throw new Error("missing screen");
  await act(async () => {
    surface.focus();
    for (const [type, key, code, repeat] of [
      ["keydown", "Control", "ControlLeft", false],
      ["keydown", "Control", "ControlRight", false],
      ["keyup", "Control", "ControlLeft", false],
      ["keydown", "a", "KeyA", false],
      ["keydown", "a", "KeyA", true],
      ["keyup", "a", "KeyA", false],
      ["keyup", "Control", "ControlRight", false],
    ] as const) {
      surface.dispatchEvent(
        new KeyboardEvent(type, { key, code, repeat, bubbles: true }),
      );
    }
    surface.blur();
  });
  await settle();
  expect(f.inputs.flat().map((e) => e.input)).toEqual([
    { type: "key", key: "Control", pressed: true },
    { type: "key", key: "a", pressed: true },
    { type: "key", key: "a", pressed: true },
    { type: "key", key: "a", pressed: false },
    { type: "key", key: "Control", pressed: false },
    { type: "releaseAll" },
  ]);
});
test("被控端只在点击允许后批准，持续提供结束操作", async () => {
  const f = fake();
  const view = await render(
    <RemoteBanner
      source={f.source}
      view={{ sessions: [session] }}
      ownId="host"
      onOpen={() => {}}
    />,
  );
  expect(f.calls).toEqual([]);
  await click(button(view.host, "允许"));
  expect(f.calls).toEqual(["approve"]);
  await click(button(view.host, "拒绝"));
  expect(f.calls).toEqual(["approve", "end"]);
});
test("StrictMode 和语言切换不结束会话，等待批准时失焦不会发送输入，卸载后才关闭", async () => {
  const f = fake();
  const view = await render(
    <StrictMode>
      <RemotePage
        source={f.source}
        view={{ sessions: [session] }}
        devices={[]}
        ownId="controller"
        onSendFiles={() => {}}
      />
    </StrictMode>,
  );
  await settle();
  expect(f.calls).toEqual([]);
  await act(async () => {
    window.dispatchEvent(new Event("blur"));
  });
  expect(f.inputs).toEqual([]);
  await act(async () => {
    await setLanguage("en");
  });
  await settle();
  expect(f.calls).toEqual([]);
  await view.unmount();
  await settle();
  expect(f.calls).toEqual(["end"]);
});
