// 按官方 Date Picker 的输入框示例组合，不调用浏览器的原生日期/时间面板。
import { useDisplayPreferences } from "@assemble/display";
import { format } from "date-fns";
import { enUS, zhCN } from "date-fns/locale";
import { CalendarIcon } from "lucide-react";
import { useState } from "react";
import { parseCalendarDate } from "../lib/date-input";
import { useUiTranslation } from "../lib/i18n";
import { Button } from "./button";
import { Calendar } from "./calendar";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./controls/select";
import { Field, FieldGroup, FieldLabel } from "./field";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "./input-group";
import { Popover, PopoverContent, PopoverTrigger } from "./popover";

const HOURS = Array.from({ length: 24 }, (_, n) => String(n).padStart(2, "0"));
const MINUTES = Array.from({ length: 60 }, (_, n) =>
  String(n).padStart(2, "0"),
);

export function DatePicker({
  id,
  label,
  value,
  onChange,
  disabled = false,
  withTime = false,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  withTime?: boolean;
}) {
  const { t } = useUiTranslation();
  const { language } = useDisplayPreferences();
  const [open, setOpen] = useState(false);
  const [month, setMonth] = useState<Date>();
  const dateText = value.split("T")[0];
  const selected = parseCalendarDate(dateText);
  const rawTime = value.split("T")[1] ?? "";
  const time = /^(?:[01]\d|2[0-3]):[0-5]\d$/.test(rawTime) ? rawTime : "00:00";
  const [hour, minute] = time.split(":");

  function changeTime(nextHour: string, nextMinute: string) {
    onChange(`${dateText}T${nextHour}:${nextMinute}`);
  }

  return (
    <Popover
      open={open && !disabled}
      onOpenChange={(next) => {
        if (next) setMonth(selected);
        setOpen(next);
      }}
    >
      <InputGroup>
        <InputGroupInput
          id={id}
          value={withTime ? value.replace("T", " ") : value}
          placeholder={t(withTime ? "datePicker.dateTime" : "datePicker.date")}
          disabled={disabled}
          autoComplete="off"
          spellCheck={false}
          onChange={(event) => {
            const next = withTime
              ? event.target.value.replace(" ", "T")
              : event.target.value;
            onChange(next);
            const date = parseCalendarDate(next.split("T")[0]);
            if (date) setMonth(date);
          }}
        />
        <InputGroupAddon align="inline-end">
          <PopoverTrigger asChild>
            <InputGroupButton
              type="button"
              variant="ghost"
              size="icon-xs"
              disabled={disabled}
              aria-label={t("datePicker.choose", { label })}
            >
              <CalendarIcon />
            </InputGroupButton>
          </PopoverTrigger>
        </InputGroupAddon>
      </InputGroup>
      <PopoverContent className="w-auto p-0" align="start" aria-label={label}>
        <Calendar
          mode="single"
          locale={language === "en" ? enUS : zhCN}
          labels={{
            labelNav: () => t("datePicker.navigation"),
            labelPrevious: () => t("datePicker.previousMonth"),
            labelNext: () => t("datePicker.nextMonth"),
            labelDayButton: (date, modifiers) =>
              [
                modifiers.today ? t("datePicker.today") : null,
                new Intl.DateTimeFormat(language, {
                  year: "numeric",
                  month: "long",
                  day: "numeric",
                  weekday: "long",
                }).format(date),
                modifiers.selected ? t("datePicker.selected") : null,
              ]
                .filter(Boolean)
                .join(", "),
          }}
          selected={selected}
          month={month}
          onMonthChange={setMonth}
          onSelect={(date) => {
            onChange(
              date
                ? `${format(date, "yyyy-MM-dd")}${withTime ? `T${time}` : ""}`
                : "",
            );
            if (!withTime) setOpen(false);
          }}
          autoFocus
        />
        {withTime && (
          <FieldGroup className="grid grid-cols-2 gap-2 px-2">
            {(
              [
                ["hour", hour, HOURS],
                ["minute", minute, MINUTES],
              ] as const
            ).map(([part, current, options]) => (
              <Field key={part}>
                <FieldLabel htmlFor={`${id}-${part}`}>
                  {t(`datePicker.${part}`)}
                </FieldLabel>
                <Select
                  value={current}
                  disabled={disabled}
                  onValueChange={(next) =>
                    part === "hour"
                      ? changeTime(next, minute)
                      : changeTime(hour, next)
                  }
                >
                  <SelectTrigger id={`${id}-${part}`} className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      {options.map((option) => (
                        <SelectItem key={option} value={option}>
                          {option}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
            ))}
          </FieldGroup>
        )}
        <div className="flex justify-between gap-2 p-2">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={disabled || !value}
            onClick={() => {
              onChange("");
              setOpen(false);
            }}
          >
            {t("datePicker.clear")}
          </Button>
          <Button type="button" size="sm" onClick={() => setOpen(false)}>
            {t("datePicker.done")}
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  );
}
