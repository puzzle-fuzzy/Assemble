// 日程卡片里的表单：新建或修改一条日程、一条提醒。
//
// 这里只管收集。填得对不对，Rust 还会按服务端同一套规则校验一次；
// 这边先把最常见的几种填错说清楚，省得等一个来回。
// 时间在输入框里是本地时间，交出去之前换成 UTC。

import { DatePicker } from "@assemble/ui/date-picker";
import {
  FieldError,
  FieldGroup,
  FieldLabel,
  Field as FormField,
} from "@assemble/ui/field";
import { type ReactNode, useId, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { uiText, useDesktopTranslation } from "@/lib/i18n";
import {
  fromLocalInput,
  leadLabel,
  leadOptions,
  problemWith,
  repeatLabel,
  toLocalInput,
} from "@/lib/planner-text";
import type { ReminderView, Repeat, ScheduleView } from "@/lib/types";

/** 正在填的表单；新建的没有 `original`。 */
export type Form =
  | { kind: "schedule"; original: ScheduleView | null }
  | { kind: "reminder"; original: ReminderView | null };

/** 表单填好之后交出去的东西：时间已经换成 UTC，没填的可选项是 null。 */
export interface Draft {
  title: string;
  /** 日程的开始时间，或者提醒的到点时间。 */
  start: string;
  end: string | null;
  repeat: Repeat;
  remindBefore: number | null;
  /** 地点的名字；没填是空串。 */
  place: string;
  notes: string | null;
}

/** 日程可以怎么重复，下拉框里的次序。 */
const REPEATS: ReadonlyArray<Repeat> = ["none", "daily", "weekly", "monthly"];

export function PlannerForm({
  form,
  pending,
  onSave,
  onClose,
  onRemove,
}: {
  form: Form;
  pending: boolean;
  onSave: (draft: Draft) => Promise<void>;
  onClose: () => void;
  /** 删掉正在修改的这一条；新建的表单没有这一项。 */
  onRemove: (() => Promise<void>) | null;
}) {
  useDesktopTranslation();
  const schedule = form.kind === "schedule" ? form.original : null;
  const reminder = form.kind === "reminder" ? form.original : null;
  const [title, setTitle] = useState(schedule?.title ?? reminder?.title ?? "");
  const [at, setAt] = useState(
    toLocalInput(schedule?.startAt ?? reminder?.dueAt ?? ""),
  );
  const [end, setEnd] = useState(
    schedule?.endAt ? toLocalInput(schedule.endAt) : "",
  );
  const [repeat, setRepeat] = useState<Repeat>(schedule?.repeat ?? "none");
  // 下拉框的值是文字：空串就是"到点时提醒"。
  const [lead, setLead] = useState(String(schedule?.remindBefore ?? ""));
  const [place, setPlace] = useState(schedule?.location?.label ?? "");
  const [notes, setNotes] = useState(schedule?.notes ?? reminder?.notes ?? "");
  const [invalidDraft, setInvalidDraft] = useState<
    Parameters<typeof problemWith>[0] | null
  >(null);
  const problem = invalidDraft
    ? (problemWith(invalidDraft) ?? uiText("enterACompleteDateAndTime"))
    : null;
  const [confirming, setConfirming] = useState(false);

  // 助手定下的提前量不在常选的那几个里：照样列出来，不改它就不会丢。
  const chosen = schedule?.remindBefore ?? null;
  const availableLeads = leadOptions();
  const leads: ReadonlyArray<readonly [number | null, string]> =
    chosen !== null && !availableLeads.some(([minutes]) => minutes === chosen)
      ? [...availableLeads, [chosen, leadLabel(chosen)]]
      : availableLeads;
  const heading =
    form.kind === "schedule"
      ? schedule
        ? uiText("editSchedule")
        : uiText("newSchedule")
      : reminder
        ? uiText("editReminder")
        : uiText("newReminder");

  function submit() {
    const issue = problemWith({ title, at, end });
    const start = fromLocalInput(at);
    if (issue !== null || start === null) {
      setInvalidDraft({ title, at, end });
      return;
    }
    setInvalidDraft(null);
    // 旧日程可能精确到秒/毫秒；只改标题时，分钟选择器不能顺手抹掉未编辑的精度。
    const originalStart = schedule?.startAt ?? reminder?.dueAt;
    const originalEnd = schedule?.endAt;
    void onSave({
      title: title.trim(),
      start:
        originalStart && at === toLocalInput(originalStart)
          ? originalStart
          : start,
      end:
        originalEnd && end === toLocalInput(originalEnd)
          ? originalEnd
          : end
            ? fromLocalInput(end)
            : null,
      repeat,
      remindBefore: lead === "" ? null : Number(lead),
      place: place.trim(),
      notes: notes.trim() || null,
    });
  }

  return (
    <form
      aria-label={heading}
      className="flex flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      <h2 className="text-sm font-medium">{heading}</h2>
      <FieldGroup>
        <Field
          label={uiText("title")}
          control={(id) => (
            <Input
              id={id}
              value={title}
              maxLength={200}
              autoFocus
              disabled={pending}
              onChange={(event) => setTitle(event.target.value)}
            />
          )}
        />
        <FieldGroup className="grid grid-cols-1 gap-3 @md:grid-cols-2">
          <Field
            label={
              form.kind === "schedule" ? uiText("startTime") : uiText("dueTime")
            }
            control={(id) => (
              <DatePicker
                id={id}
                label={
                  form.kind === "schedule"
                    ? uiText("startTime")
                    : uiText("dueTime")
                }
                withTime
                value={at}
                disabled={pending}
                onChange={setAt}
              />
            )}
          />
          {form.kind === "schedule" ? (
            <Field
              label={uiText("endTime")}
              control={(id) => (
                <DatePicker
                  id={id}
                  label={uiText("endTime")}
                  withTime
                  value={end}
                  disabled={pending}
                  onChange={setEnd}
                />
              )}
            />
          ) : null}
        </FieldGroup>
        {form.kind === "schedule" ? (
          <>
            <FieldGroup className="grid grid-cols-1 gap-3 @md:grid-cols-2">
              <Field
                label={uiText("repeat")}
                control={(id) => (
                  <Select
                    value={repeat}
                    disabled={pending}
                    onValueChange={(value) => setRepeat(value as Repeat)}
                  >
                    <SelectTrigger id={id} className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {REPEATS.map((option) => (
                          <SelectItem key={option} value={option}>
                            {repeatLabel(option)}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                )}
              />
              <Field
                label={uiText("notification")}
                control={(id) => (
                  <Select
                    value={lead || "at-time"}
                    disabled={pending}
                    onValueChange={(value) =>
                      setLead(value === "at-time" ? "" : value)
                    }
                  >
                    <SelectTrigger id={id} className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {leads.map(([minutes, text]) => (
                          <SelectItem
                            key={text}
                            value={
                              minutes === null ? "at-time" : String(minutes)
                            }
                          >
                            {text}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                )}
              />
            </FieldGroup>
            <Field
              label={uiText("location")}
              control={(id) => (
                <Input
                  id={id}
                  value={place}
                  maxLength={200}
                  disabled={pending}
                  onChange={(event) => setPlace(event.target.value)}
                />
              )}
            />
          </>
        ) : null}
        <Field
          label={uiText("notes")}
          control={(id) => (
            <Textarea
              id={id}
              value={notes}
              maxLength={4000}
              disabled={pending}
              onChange={(event) => setNotes(event.target.value)}
            />
          )}
        />
      </FieldGroup>
      {problem ? <FieldError>{problem}</FieldError> : null}
      <div className="flex flex-wrap gap-2">
        <Button type="submit" disabled={pending}>
          {uiText("save")}
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={pending}
          onClick={onClose}
        >
          {uiText("cancel")}
        </Button>
        {onRemove ? (
          confirming ? (
            <>
              <Button
                type="button"
                variant="destructive"
                disabled={pending}
                onClick={() => void onRemove()}
              >
                {uiText("confirmDeletion")}
              </Button>
              <Button
                type="button"
                variant="outline"
                disabled={pending}
                onClick={() => setConfirming(false)}
              >
                {uiText("neverMind")}
              </Button>
            </>
          ) : (
            <Button
              type="button"
              variant="outline"
              disabled={pending}
              onClick={() => setConfirming(true)}
            >
              {uiText("delete")}
            </Button>
          )
        ) : null}
      </div>
    </form>
  );
}

/**
 * 一栏：标签和控件用同一个编号连起来，读屏念得出这是哪一项，点文字也能把焦点放进去。
 * 控件自己决定是什么，只要把 `id` 挂上去。
 */
function Field({
  label,
  control,
}: {
  label: string;
  control: (id: string) => ReactNode;
}) {
  const id = useId();
  return (
    <FormField className="min-w-0">
      <FieldLabel htmlFor={id}>{label}</FieldLabel>
      {control(id)}
    </FormField>
  );
}
