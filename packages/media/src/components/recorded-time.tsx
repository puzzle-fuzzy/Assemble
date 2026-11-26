import { useMediaTranslation } from "../i18n";

export function MediaRecordedTime({ value }: { value: string }) {
  const { t, i18n } = useMediaTranslation();
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return <>{t("common.unknown")}</>;
  return (
    <time dateTime={value}>
      {new Intl.DateTimeFormat(i18n.resolvedLanguage || i18n.language, {
        timeZone: "Asia/Shanghai",
        year: "numeric",
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
        hourCycle: "h23",
      }).format(date)}
    </time>
  );
}
