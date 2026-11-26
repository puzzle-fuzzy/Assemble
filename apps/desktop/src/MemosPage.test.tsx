import { afterEach, beforeEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import { App } from "@/App";
import { BridgeError } from "@/lib/bridge";
import type {
  Memo,
  MemoBridge,
  MemoContent,
  MemoList,
  MemoQuery,
} from "@/lib/memos";
import { MemosPage } from "@/MemosPage";
import {
  button,
  cleanup,
  click,
  field,
  fill,
  render,
  settle,
  text,
} from "@/test-support/dom";
import { ACCOUNT, FakeBridge, signedIn, snapshot } from "@/test-support/fakes";

beforeEach(() => setLanguage("zh-CN"));
afterEach(async () => {
  await cleanup();
  setLanguage("zh-CN");
});

const original: Memo = {
  id: "memo-first",
  version: 1,
  title: "常用文本",
  text: `  第一行\n<script>不是网页</script>\n${"完整原文".repeat(70)}`,
  createdAt: "2026-11-26T01:02:03.000Z",
  updatedAt: "2026-11-26T01:02:03.000Z",
};
function fake() {
  const state = {
    account: "account-1",
    records: new Map<string, Memo[]>([
      ["account-1", [original]],
      ["account-2", []],
    ]),
    calls: [] as { action: string; value: unknown }[],
    failCopy: false,
    failCreate: false,
    failList: false,
  };
  const read = (id: string) => {
    const record = state.records
      .get(state.account)
      ?.find((memo) => memo.id === id);
    if (!record) throw new BridgeError("NOT_FOUND", "private diagnostic");
    return record;
  };
  const source: MemoBridge = {
    async list(query) {
      state.calls.push({ action: "list", value: { ...query } });
      if (state.failList)
        throw new BridgeError("NETWORK", "private diagnostic");
      const rows = (state.records.get(state.account) ?? []).filter((memo) =>
        `${memo.title} ${memo.text}`.includes(query.q),
      );
      return {
        items: rows
          .slice((query.page - 1) * query.pageSize, query.page * query.pageSize)
          .map(({ text, ...memo }) => ({
            ...memo,
            preview: text.slice(0, 200),
          })),
        total: rows.length,
        page: query.page,
        pageSize: query.pageSize,
      };
    },
    async get(id) {
      state.calls.push({ action: "get", value: id });
      return read(id);
    },
    async create(content, requestId) {
      state.calls.push({ action: "create", value: { content, requestId } });
      if (state.failCreate)
        throw new BridgeError("NETWORK", "private diagnostic");
      const memo = { ...original, ...content, id: requestId };
      state.records.get(state.account)?.unshift(memo);
      return memo;
    },
    async update(id, version, content) {
      state.calls.push({ action: "update", value: { id, version, content } });
      const previous = read(id);
      if (version !== previous.version)
        throw new BridgeError("VERSION_CONFLICT", "private diagnostic");
      const memo = { ...previous, ...content, version: version + 1 };
      state.records.set(
        state.account,
        (state.records.get(state.account) ?? []).map((item) =>
          item.id === id ? memo : item,
        ),
      );
      return memo;
    },
    async remove(id, version) {
      state.calls.push({ action: "remove", value: { id, version } });
      state.records.set(
        state.account,
        (state.records.get(state.account) ?? []).filter(
          (memo) => memo.id !== id,
        ),
      );
    },
    async copy(id) {
      if (state.failCopy)
        throw new BridgeError("SYSTEM_REJECTED", "private clipboard error");
      state.calls.push({ action: "copy", value: { id, text: read(id).text } });
    },
  };
  return { state, source };
}
const noAccess = () => {};
async function submit() {
  const form = document.body.querySelector('[role="dialog"] form');
  if (!form) throw new Error("memo form missing");
  await act(async () =>
    form.dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    ),
  );
  await settle();
}
test("桌面备忘录从菜单进入，未登录不读取个人数据", async () => {
  const { source, state } = fake();
  const view = await render(
    <App bridge={new FakeBridge(snapshot())} memoBridge={source} />,
  );
  await settle();
  await click(button(view.host, "备忘录"));
  await settle();
  expect(text(view.host)).toContain("登录后使用备忘录");
  expect(state.calls).toHaveLength(0);
  await click(button(view.host, "前往账户中心"));
  expect(
    view.host.querySelector('[data-slot="breadcrumb-page"]')?.textContent,
  ).toBe("账户中心");
});
test("新建纯文本保留粘贴内容和失败草稿，列表直接复制完整原文并打开详情", async () => {
  const { source, state } = fake();
  const view = await render(
    <MemosPage source={source} accountId="account-1" onSignIn={noAccess} />,
  );
  await settle();
  await click(button(view.host, "复制：常用文本"));
  expect(state.calls.at(-1)).toEqual({
    action: "copy",
    value: { id: original.id, text: original.text },
  });
  const trigger = button(view.host, "详情：常用文本");
  if (!trigger) throw new Error("memo details action missing");
  await act(async () => trigger?.focus());
  await click(trigger);
  expect((field(document.body, "文本内容") as HTMLTextAreaElement).value).toBe(
    original.text,
  );
  expect(document.body.querySelector("script")).toBeNull();
  await click(button(document.body, "关闭"));
  expect(document.activeElement).toBe(trigger);
  await click(button(view.host, "新建备忘录"));
  const content: MemoContent = {
    title: "粘贴文本",
    text: " 前后空格 \n\t带换行🙂\n",
  };
  await fill(document.body, "标题", content.title);
  await fill(document.body, "文本内容", content.text);
  state.failCreate = true;
  await submit();
  expect(document.body.textContent).toContain("操作未完成，请重试");
  expect((field(document.body, "文本内容") as HTMLTextAreaElement).value).toBe(
    content.text,
  );
  const before = state.calls.at(-1)?.value;
  state.failCreate = false;
  await submit();
  expect(
    state.calls.filter((call) => call.action === "create").at(-1)?.value,
  ).toEqual(before);
  expect(state.records.get("account-1")?.[0].text).toBe(content.text);
  expect(view.host.textContent).toContain(content.title);
});
test("账号切换卸载旧草稿和内容，切回恢复原账号备忘录", async () => {
  const { source, state } = fake();
  const auth = new FakeBridge(snapshot({ phase: "signedIn" }));
  const view = await render(<App bridge={auth} memoBridge={source} />);
  await settle();
  await click(button(view.host, "备忘录"));
  await settle();
  expect(view.host.textContent).toContain(original.title);
  await click(button(view.host, "新建备忘录"));
  await fill(document.body, "文本内容", "未保存草稿");
  state.account = "account-2";
  await act(async () =>
    auth.push({
      ...snapshot({ phase: "signedIn" }),
      session: signedIn({
        ...ACCOUNT,
        id: "account-2",
        displayName: "第二个账号",
      }),
    }),
  );
  await settle();
  expect(document.body.textContent).not.toContain("未保存草稿");
  expect(view.host.textContent).not.toContain(original.title);
  expect(view.host.textContent).toContain("还没有备忘录");
  state.account = "account-1";
  await act(async () => auth.push(snapshot({ phase: "signedIn" })));
  await settle();
  expect(view.host.textContent).toContain(original.title);
});
test("搜索重置分页，双语不重读或修改正文，复制失败可见且可以重试", async () => {
  const { source, state } = fake();
  state.records.set(
    "account-1",
    Array.from({ length: 21 }, (_, index) => ({
      ...original,
      id: `memo-${index}`,
      title: `第${index}条`,
    })),
  );
  const view = await render(
    <MemosPage source={source} accountId="account-1" onSignIn={noAccess} />,
  );
  await settle();
  await click(button(view.host, "下一页"));
  expect((state.calls.at(-1)?.value as MemoQuery | undefined)?.page).toBe(2);
  await fill(view.host, "搜索备忘录", "第0条");
  const form = view.host.querySelector("form");
  await act(async () =>
    form?.dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    ),
  );
  await settle();
  expect(state.calls.at(-1)?.value).toEqual({
    q: "第0条",
    page: 1,
    pageSize: 10,
  });
  const count = state.calls.length;
  await act(async () => setLanguage("en"));
  expect(state.calls).toHaveLength(count);
  expect(view.host.textContent).toContain("Preview");
  state.failCopy = true;
  await click(button(view.host, "Copy: 第0条"));
  expect(view.host.textContent).toContain("Copy failed");
  expect(view.host.textContent).not.toContain("private clipboard error");
  state.failCopy = false;
  await click(button(view.host, "Copy: 第0条"));
  expect(state.calls.at(-1)?.action).toBe("copy");
});
test("编辑冲突保留输入，删除必须确认，读取失败可重试", async () => {
  const { source, state } = fake();
  state.failList = true;
  const view = await render(
    <MemosPage source={source} accountId="account-1" onSignIn={noAccess} />,
  );
  await settle();
  expect(view.host.textContent).toContain("暂时无法读取备忘录");
  state.failList = false;
  await click(button(view.host, "重试"));
  await click(button(view.host, "编辑：常用文本"));
  await fill(document.body, "文本内容", "本机修改");
  state.records.set("account-1", [
    { ...original, version: 2, text: "别处修改" },
  ]);
  await submit();
  expect(document.body.textContent).toContain("已在其他设备修改");
  expect((field(document.body, "文本内容") as HTMLTextAreaElement).value).toBe(
    "本机修改",
  );
  await click(button(document.body, "取消"));
  await click(button(view.host, "删除：常用文本"));
  await click(button(document.body, "取消"));
  expect(state.calls.some((call) => call.action === "remove")).toBe(false);
  await click(button(view.host, "刷新"));
  await click(button(view.host, "删除：常用文本"));
  const confirmation = document.body.querySelector<HTMLElement>(
    '[role="alertdialog"]',
  );
  if (!confirmation) throw new Error("memo delete confirmation missing");
  await click(button(confirmation, "删除"));
  expect(state.calls.find((call) => call.action === "remove")?.value).toEqual({
    id: original.id,
    version: 2,
  });
  expect(view.host.textContent).toContain("还没有备忘录");
});
test("账号卸载与旧查询取消后，迟到响应不能恢复私人内容", async () => {
  const { source } = fake();
  const gate = Promise.withResolvers<MemoList>();
  let signal: AbortSignal | undefined;
  source.list = async (_query, incoming) => {
    signal = incoming;
    return gate.promise;
  };
  const view = await render(
    <MemosPage source={source} accountId="account-1" onSignIn={noAccess} />,
  );
  await settle();
  const obsolete = signal;
  await cleanup();
  expect(obsolete?.aborted).toBe(true);
  await act(async () =>
    gate.resolve({
      items: [{ ...original, preview: original.text }],
      total: 1,
      page: 1,
      pageSize: 10,
    }),
  );
  expect(document.body.textContent).not.toContain(original.title);
  expect(view.host.isConnected).toBe(false);
});
