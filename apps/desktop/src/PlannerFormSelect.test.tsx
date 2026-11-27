import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act } from "react";
import {
  chooseSelect,
  selectTrigger,
} from "../../../packages/ui/src/test-support/select";
import { type Draft, PlannerForm } from "./PlannerForm";
import { button, cleanup, click, fill, render } from "./test-support/dom";

afterEach(async () => {
  await cleanup();
  await act(async () => setLanguage("zh-CN"));
});

test("日程选择不触发保存，语言切换保留选项，到点时提醒仍交出 null", async () => {
  let saved: Draft | undefined;
  const { host } = await render(
    <PlannerForm
      form={{ kind: "schedule", original: null }}
      pending={false}
      onSave={async (draft) => {
        saved = draft;
      }}
      onClose={() => {}}
      onRemove={null}
    />,
  );
  expect(selectTrigger(host, "重复").textContent).toContain("不重复");
  expect(selectTrigger(host, "提醒").textContent).toContain("到点时提醒");
  await chooseSelect(host, "重复", "每周");
  await chooseSelect(host, "提醒", "提前 1 小时");
  await chooseSelect(host, "提醒", "到点时提醒");
  expect(saved).toBeUndefined();
  await act(async () => setLanguage("en"));
  expect(selectTrigger(host, "Repeat").textContent).toContain("Weekly");
  expect(selectTrigger(host, "Notification").textContent).toContain(
    "At the scheduled time",
  );
  await fill(host, "Title", "保留草稿");
  await fill(host, "Start time", "2026-11-28 09:07");
  await click(button(host, "Save"));
  expect(saved).toMatchObject({
    title: "保留草稿",
    repeat: "weekly",
    remindBefore: null,
  });
});

test("保存中的日程选择器维持禁用状态", async () => {
  const { host } = await render(
    <PlannerForm
      form={{ kind: "schedule", original: null }}
      pending
      onSave={async () => {
        throw new Error("UNEXPECTED_SAVE");
      }}
      onClose={() => {}}
      onRemove={null}
    />,
  );
  expect(selectTrigger(host, "重复").disabled).toBe(true);
  expect(selectTrigger(host, "提醒").disabled).toBe(true);
});
