const formatter = new Intl.DateTimeFormat("zh-CN", {
  timeZone: "Asia/Shanghai",
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  hourCycle: "h23",
});

export function RecordedTime({ value }: { value: string | null }) {
  if (value === null) return <span>未记录</span>;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return <span>未记录</span>;
  return <time dateTime={value}>{formatter.format(date)}</time>;
}

export const platformName = {
  windows: "Windows",
  macos: "macOS",
  linux: "Linux",
  other: "其他",
};
