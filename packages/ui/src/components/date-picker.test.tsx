import { afterEach, expect, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { DatePicker } from "./date-picker";
import { Field, FieldLabel } from "./field";

let root: Root;
let host: HTMLDivElement;
let submissions = 0;
let savedValue = "";

afterEach(async () => {
  await act(async () => {
    root?.unmount();
    setLanguage("zh-CN");
  });
  host?.remove();
});

function PickerForm({
  value,
  withTime,
  disabled,
}: {
  value: string;
  withTime: boolean;
  disabled: boolean;
}) {
  const [current, setCurrent] = useState(value);
  savedValue = current;
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        submissions++;
      }}
    >
      <Field>
        <FieldLabel htmlFor="date">日期</FieldLabel>
        <DatePicker
          id="date"
          label="日期"
          value={current}
          withTime={withTime}
          disabled={disabled}
          onChange={setCurrent}
        />
      </Field>
    </form>
  );
}

async function mount(value: string, withTime = false, disabled = false) {
  submissions = 0;
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () =>
    root.render(
      <PickerForm value={value} withTime={withTime} disabled={disabled} />,
    ),
  );
}

function button(name: string) {
  const found = [
    ...document.querySelectorAll<HTMLButtonElement>("button"),
  ].find(
    (element) =>
      (element.getAttribute("aria-label") ?? element.textContent?.trim()) ===
      name,
  );
  if (!found) throw new Error(`BUTTON_MISSING: ${name}`);
  return found;
}

async function click(element: HTMLElement) {
  await act(async () => element.click());
}

function day(dayOfMonth: number) {
  const found = document.querySelector<HTMLButtonElement>(
    `button[data-day="${new Date(2026, 10, dayOfMonth).toLocaleDateString("zh-CN")}"]`,
  );
  if (!found) throw new Error("CALENDAR_DAY_MISSING");
  return found;
}

function input() {
  return host.querySelector<HTMLInputElement>("#date");
}

async function chooseTime(part: "hour" | "minute", value: string) {
  const trigger = document.getElementById(`date-${part}`);
  if (!trigger) throw new Error("TIME_TRIGGER_MISSING");
  await act(async () => {
    trigger.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
    );
  });
  const end = performance.now() + 2000;
  let option: HTMLElement | undefined;
  while (!option) {
    option = [
      ...document.querySelectorAll<HTMLElement>('[role="option"]'),
    ].find((element) => element.textContent?.trim() === value);
    if (option) break;
    if (performance.now() > end) throw new Error("TIME_OPTION_MISSING");
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
  }
  await click(option);
}

test("日期采用官方日历，选择和清除不会提交表单或把日期换算成 UTC", async () => {
  await mount("2026-11-25");
  expect(input()?.type).toBe("text");
  expect(
    document.querySelector(
      'input[type="date"], input[type="time"], input[type="datetime-local"]',
    ),
  ).toBeNull();
  const trigger = button("选择日期");
  await click(trigger);
  expect(day(25).dataset.selectedSingle).toBe("true");
  expect(day(25).getAttribute("aria-label")).toContain("已选中");
  expect(button("上个月")).toBeTruthy();
  await click(day(26));
  expect(savedValue).toBe("2026-11-26");
  expect(input()?.value).toBe("2026-11-26");
  expect(trigger.getAttribute("aria-expanded")).toBe("false");
  await click(trigger);
  await click(button("清除"));
  expect(savedValue).toBe("");
  expect(submissions).toBe(0);
});

test("日期时间通过 shadcn 下拉选择到每分钟，更改日期保留原时刻", async () => {
  await mount("2026-11-25T09:07", true);
  await click(button("选择日期"));
  await click(day(26));
  expect(savedValue).toBe("2026-11-26T09:07");
  await chooseTime("hour", "23");
  await chooseTime("minute", "59");
  expect(savedValue).toBe("2026-11-26T23:59");
  expect(input()?.value).toBe("2026-11-26 23:59");
  await click(button("完成"));
  expect(submissions).toBe(0);
  expect(host.querySelector('[aria-expanded="true"]')).toBeNull();
});

test("允许直接输入较早日期、不完整草稿交给现有表单校验", async () => {
  await mount("");
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set?.call(input(), "1897-08-01");
    input()?.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await click(button("选择日期"));
  expect(
    document.querySelector('[data-slot="calendar"]')?.textContent,
  ).toContain("1897");
  expect(savedValue).toBe("1897-08-01");
  await click(button("完成"));
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set?.call(input(), "2026-02-");
    input()?.dispatchEvent(new Event("input", { bubbles: true }));
  });
  expect(savedValue).toBe("2026-02-");
  expect(submissions).toBe(0);
});

test("切换语言翻译日历和时刻标签，保持当前草稿；禁用时没有可打开的原生面板", async () => {
  await mount("2026-11-25T09:07", true);
  await click(button("选择日期"));
  await act(async () => setLanguage("en"));
  expect(
    document.querySelector('[data-slot="calendar"]')?.textContent,
  ).toContain("November");
  expect(button("Previous month")).toBeTruthy();
  expect(document.querySelector('label[for="date-hour"]')?.textContent).toBe(
    "Hour",
  );
  expect(document.querySelector('label[for="date-minute"]')?.textContent).toBe(
    "Minute",
  );
  expect(savedValue).toBe("2026-11-25T09:07");
  await click(button("Done"));
  await act(async () =>
    root.render(
      <DatePicker
        id="locked"
        label="日期"
        value={savedValue}
        onChange={() => {
          throw new Error("DISABLED_CHANGE");
        }}
        disabled
        withTime
      />,
    ),
  );
  expect(host.querySelector<HTMLInputElement>("#locked")?.disabled).toBe(true);
  expect(button("Choose 日期").disabled).toBe(true);
  expect(document.querySelector('[data-slot="calendar"]')).toBeNull();
});
