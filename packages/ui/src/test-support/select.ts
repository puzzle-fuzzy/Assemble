// 驱动真实 shadcn 弹层，测试不再伪造原生 select 的 change 事件。
import { act } from "react";

export function selectTrigger(
  root: ParentNode,
  label: string,
): HTMLButtonElement {
  const tag = [...root.querySelectorAll<HTMLLabelElement>("label")].find(
    (element) => element.textContent?.trim() === label,
  );
  const trigger = tag?.control;
  if (
    !(trigger instanceof HTMLButtonElement) ||
    trigger.getAttribute("role") !== "combobox"
  )
    throw new Error(`SELECT_MISSING: ${label}`);
  return trigger;
}

export async function chooseSelect(
  root: ParentNode,
  label: string,
  option: string,
) {
  const trigger = selectTrigger(root, label);
  await act(async () => {
    trigger.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
    );
  });
  const deadline = performance.now() + 2000;
  for (;;) {
    const item = [
      ...document.querySelectorAll<HTMLElement>('[role="option"]'),
    ].find((element) => element.textContent?.trim() === option);
    if (item) {
      if (item.getAttribute("aria-disabled") === "true")
        throw new Error(`SELECT_OPTION_DISABLED: ${option}`);
      await act(async () => item.click());
      return;
    }
    if (performance.now() > deadline)
      throw new Error(`SELECT_OPTION_MISSING: ${option}`);
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
  }
}
