import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act, StrictMode } from "react";
import type { RemoteBridge, RemoteSession, SequencedInput } from "@/lib/remote";
import { isRemoteSession, type RemoteEndReason } from "@/lib/remote";
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
    audio: async () => ({ state: "off", packets: [] }),
    clipboard: async () => "completed",
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
function activeSource() {
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
  return { ...f, active };
}
async function activePage(f: ReturnType<typeof activeSource>) {
  const view = await render(
    <RemotePage
      source={f.source}
      view={{ sessions: [f.active] }}
      devices={[]}
      ownId="controller"
      onSendFiles={() => {}}
    />,
  );
  await settle();
  const surface = view.host.querySelector<HTMLElement>('[role="application"]');
  if (!surface) throw new Error("missing screen");
  return { ...view, surface };
}
afterEach(async () => {
  await cleanup();
  await setLanguage("zh-CN");
});

test.each([
  ["declined", "对方拒绝了控制请求。", "status"],
  ["expired", "控制请求已过期，请重新连接。", "status"],
  ["disconnected", "对方连接中断，键鼠控制已停止，请重新连接。", "alert"],
  [
    "captureFailed",
    "对方无法继续采集屏幕，请检查屏幕录制权限后重新连接。",
    "alert",
  ],
] as const)(
  "结束原因 %s 使用对应文案与公告级别",
  async (reason: RemoteEndReason, message, role) => {
    const f = fake();
    f.source.poll = async () => ({
      session: { ...session, phase: "ended", endReason: reason },
      frame: null,
      inputs: [],
    });
    const view = await render(
      <RemotePage
        source={f.source}
        view={{ sessions: [session] }}
        devices={[]}
        ownId="controller"
        onSendFiles={() => {}}
      />,
    );
    await settle();
    expect(view.host.querySelector(`[role="${role}"]`)?.textContent).toContain(
      message,
    );
  },
);
test("未知原因与活动会话里的结束原因不能通过桥接", () => {
  expect(
    isRemoteSession({ ...session, phase: "ended", endReason: "captureFailed" }),
  ).toBe(true);
  expect(
    isRemoteSession({ ...session, phase: "ended", endReason: "future" }),
  ).toBe(false);
  expect(
    isRemoteSession({ ...session, phase: "active", endReason: "stopped" }),
  ).toBe(false);
});
test("Ctrl+V 等待松键排空后才交换剪贴板，交换期间不注入其他按键", async () => {
  const f = activeSource();
  let acceptInput = () => {};
  let acceptClipboard = () => {};
  f.source.inputs = async (_id, events) => {
    f.inputs.push(events);
    if (f.inputs.length === 1)
      await new Promise<void>((resolve) => {
        acceptInput = resolve;
      });
  };
  const exchanges: unknown[][] = [];
  f.source.clipboard = async (...args) => {
    exchanges.push(args);
    await new Promise<void>((resolve) => {
      acceptClipboard = resolve;
    });
    return "completed";
  };
  const view = await activePage(f);
  await act(async () => {
    view.surface.focus();
    view.surface.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Control",
        code: "ControlLeft",
        ctrlKey: true,
        bubbles: true,
      }),
    );
    view.surface.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "v",
        code: "KeyV",
        ctrlKey: true,
        bubbles: true,
      }),
    );
  });
  expect(exchanges).toHaveLength(0);
  await act(async () => {
    acceptInput();
  });
  await settle();
  expect(exchanges).toEqual([["r1", "toHost", true]]);
  await act(async () => {
    view.surface.dispatchEvent(
      new KeyboardEvent("keydown", { key: "a", code: "KeyA", bubbles: true }),
    );
  });
  expect(f.inputs.flat().map((event) => event.input)).toEqual([
    { type: "key", key: "Control", pressed: true },
    { type: "releaseAll" },
  ]);
  await act(async () => {
    acceptClipboard();
  });
});
test("1:1 浏览只改变本地视图，Esc 返回控制后才发送键盘", async () => {
  const f = activeSource();
  const view = await activePage(f);
  await click(button(view.host, "1:1"));
  await click(button(view.host, "浏览画面"));
  await act(async () => {
    view.surface.focus();
    view.surface.dispatchEvent(
      new KeyboardEvent("keydown", { key: "a", code: "KeyA", bubbles: true }),
    );
  });
  expect(f.inputs.flat().some((event) => event.input.type === "key")).toBe(
    false,
  );
  await act(async () => {
    view.surface.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Escape",
        code: "Escape",
        bubbles: true,
      }),
    );
  });
  expect(button(view.host, "浏览画面")).toBeDefined();
  await act(async () => {
    view.surface.dispatchEvent(
      new KeyboardEvent("keydown", { key: "a", code: "KeyA", bubbles: true }),
    );
  });
  await settle();
  const keys = f.inputs.flat().filter((event) => event.input.type === "key");
  expect(keys.map((event) => event.input)).toEqual([
    { type: "key", key: "a", pressed: true },
  ]);
  expect(keys[0]?.displayId).toBe(1);
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
  });
  await settle();
  await act(async () => {
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
  expect(f.calls).toEqual([]);
  expect(button(view.host, "允许")?.disabled).toBe(true);
  const ready = await render(
    <RemoteBanner
      source={f.source}
      view={{
        sessions: [
          { ...session, verification: "1234-5678-9abc-def0-1234-5678" },
        ],
      }}
      ownId="host"
      onOpen={() => {}}
    />,
  );
  await click(button(ready.host, "允许"));
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
