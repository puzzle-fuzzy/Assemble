import { isValid, parseISO } from "date-fns";

// 日期没有时区；不用 Date 的 UTC 字符串解析，也不接受自动顺延的无效日期。
export function parseCalendarDate(value: string): Date | undefined {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return undefined;
  const date = parseISO(value);
  return isValid(date) ? date : undefined;
}
