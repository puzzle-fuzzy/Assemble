import { useMemo } from "react";
import { useAdminTranslation } from "./i18n";

export function RecordedTime({ value }: { value: string | null }) {
  const { t, i18n } = useAdminTranslation();
  const language = i18n.resolvedLanguage || i18n.language;
  // 语言只决定显示格式；统计与时间仍统一采用北京时间。
  const formatter = useMemo(
    () =>
      new Intl.DateTimeFormat(language, {
        timeZone: "Asia/Shanghai",
        year: "numeric",
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
        hourCycle: "h23",
      }),
    [language],
  );
  if (value === null) return <span>{t("common.unrecorded")}</span>;
  const date = new Date(value);
  if (Number.isNaN(date.getTime()))
    return <span>{t("common.unrecorded")}</span>;
  return (
    <time dateTime={value} title={t("common.beijingTime")}>
      {formatter.format(date)}
    </time>
  );
}
