// 日程卡片里的三种行：一条没读的通知、一条日程、一条提醒。
//
// 每一行右边的按钮都带着看到的版本号交回去。要不要先确认、能不能改，
// 按这一项是什么、现在处在什么状态决定。

import { type ReactNode, type Ref, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import type { Bridge } from "@/lib/bridge";
import { uiText, useDesktopTranslation } from "@/lib/i18n";
import {
  ago,
  laterLabel,
  reminderLine,
  scheduleLine,
} from "@/lib/planner-text";
import type { NotificationView, ReminderView, ScheduleView } from "@/lib/types";
import { cn } from "@/lib/utils";
import type { Form } from "@/PlannerForm";

/** 做一件事；返回稳定错误码，成功时为 null。 */
export type Act = (what: () => Promise<unknown>) => Promise<string | null>;

interface ItemProps<T> {
  source: Bridge;
  item: T;
  /** 这台设备现在的名字。别的设备（多半是助手）记下的，要标出来。 */
  deviceName: string;
  pending: boolean;
  act: Act;
  onEdit: (form: Form) => void;
  separated?: boolean;
}

/** 名单里的一行：左边是说清楚的一叠文字，右边是几个按钮。 */
function Row({
  title,
  by,
  actions,
  children,
  separated = false,
}: {
  title: string;
  /** 记下它的设备；就是这台电脑时为 null。 */
  by: string | null;
  actions: ReactNode;
  children: ReactNode;
  separated?: boolean;
}) {
  return (
    <li className="relative flex flex-wrap items-start gap-3 py-3">
      {separated ? <Separator className="absolute inset-x-0 top-0" /> : null}
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <h3 className="truncate text-sm font-medium" title={title}>
            {title}
          </h3>
          {by === null ? null : (
            <Badge variant="outline" className="max-w-40 truncate">
              {by}
            </Badge>
          )}
        </div>
        {children}
      </div>
      <div className="flex max-w-full shrink-0 flex-wrap justify-end gap-2">
        {actions}
      </div>
    </li>
  );
}

/** 一行底下的一句补充的话。 */
function Line({ text }: { text: string }) {
  return <p className="text-xs text-muted-foreground">{text}</p>;
}

/** 几句备注。长的先不摆全，鼠标停上去能看整段。 */
function Note({ text }: { text: string }) {
  return (
    <p
      className="mt-1 line-clamp-2 text-xs whitespace-pre-line text-muted-foreground"
      title={text}
    >
      {text}
    </p>
  );
}

/** 先确认再做的两个按钮：确认做那件不好收回的事，或者"算了"退回去。 */
function Confirming({
  label,
  pending,
  onConfirm,
  onBail,
}: {
  label: string;
  pending: boolean;
  onConfirm: () => void;
  onBail: () => void;
}) {
  return (
    <>
      <Button
        size="sm"
        variant="destructive"
        disabled={pending}
        onClick={onConfirm}
      >
        {label}
      </Button>
      <Button size="sm" variant="outline" disabled={pending} onClick={onBail}>
        {uiText("neverMind")}
      </Button>
    </>
  );
}

/** 一条没读的通知。它对应的那条提醒还在名单上时，顺手就能看完。 */
export function NotificationRow({
  notification,
  reminder,
  highlighted,
  pending,
  onRead,
  onDone,
  ref,
  separated = false,
}: {
  notification: NotificationView;
  /** 它对应的那条提醒；名单里找得到时才有。 */
  reminder: ReminderView | null;
  /** 点开系统通知进来的那一条：标出来。 */
  highlighted: boolean;
  pending: boolean;
  onRead: () => void;
  onDone: (reminder: ReminderView) => void;
  ref?: Ref<HTMLLIElement>;
  separated?: boolean;
}) {
  useDesktopTranslation();
  return (
    <li
      ref={ref}
      aria-current={highlighted ? "true" : undefined}
      className={cn(
        "relative flex flex-wrap items-start gap-3 py-3",
        highlighted && "bg-accent",
      )}
    >
      {separated ? <Separator className="absolute inset-x-0 top-0" /> : null}
      <div className="min-w-0 flex-1">
        <h3 className="text-sm font-medium">{notification.title}</h3>
        {notification.body ? (
          <p className="line-clamp-3 text-xs whitespace-pre-line text-muted-foreground">
            {notification.body}
          </p>
        ) : null}
        <p className="text-xs text-muted-foreground">
          {ago(notification.raisedAt, Date.now())}
        </p>
      </div>
      <div className="flex max-w-full shrink-0 flex-wrap gap-2">
        {reminder ? (
          // 这条提醒已经到点了：看完就一起收掉，不用再去下面的名单里找它。
          <Button size="sm" disabled={pending} onClick={() => onDone(reminder)}>
            {uiText("done")}
          </Button>
        ) : null}
        <Button size="sm" variant="outline" disabled={pending} onClick={onRead}>
          {uiText("dismiss")}
        </Button>
      </div>
    </li>
  );
}

export function ScheduleRow({
  source,
  item,
  deviceName,
  pending,
  act,
  onEdit,
  separated,
}: ItemProps<ScheduleView>) {
  useDesktopTranslation();
  const [confirming, setConfirming] = useState(false);
  const later = laterLabel(item, Date.now());
  return (
    <Row
      separated={separated}
      title={item.title}
      by={item.createdBy === deviceName ? null : item.createdBy}
      actions={
        confirming ? (
          <Confirming
            label={uiText("confirmCancellation")}
            pending={pending}
            onConfirm={() =>
              void act(() => source.scheduleCancel(item.id, item.version))
            }
            onBail={() => setConfirming(false)}
          />
        ) : (
          <>
            {/* 重复的日程没有"办完"的那一天：要停就取消。 */}
            {item.repeat === "none" ? (
              <Button
                size="sm"
                disabled={pending}
                onClick={() =>
                  void act(() => source.scheduleComplete(item.id, item.version))
                }
              >
                {uiText("complete")}
              </Button>
            ) : null}
            <Button
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={() => onEdit({ kind: "schedule", original: item })}
            >
              {uiText("edit")}
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={() => setConfirming(true)}
            >
              {uiText("cancel")}
            </Button>
          </>
        )
      }
    >
      <Line text={scheduleLine(item, Date.now())} />
      {later === null ? null : <Line text={later} />}
      {item.location ? <Line text={item.location.label} /> : null}
      {item.notes ? <Note text={item.notes} /> : null}
    </Row>
  );
}

export function ReminderRow({
  source,
  item,
  deviceName,
  pending,
  act,
  onEdit,
  separated,
}: ItemProps<ReminderView>) {
  useDesktopTranslation();
  const [confirming, setConfirming] = useState(false);
  // 到点的提醒不能再改时间，只能看完或者不办。
  const fired = item.status === "fired";
  return (
    <Row
      separated={separated}
      title={item.title}
      by={item.createdBy === deviceName ? null : item.createdBy}
      actions={
        confirming ? (
          <Confirming
            label={uiText("confirmCancellation2")}
            pending={pending}
            onConfirm={() =>
              void act(() => source.reminderCancel(item.id, item.version))
            }
            onBail={() => setConfirming(false)}
          />
        ) : (
          <>
            <Button
              size="sm"
              disabled={pending}
              onClick={() =>
                void act(() => source.reminderDismiss(item.id, item.version))
              }
            >
              {fired ? uiText("done") : uiText("markDone")}
            </Button>
            {fired ? null : (
              <Button
                size="sm"
                variant="outline"
                disabled={pending}
                onClick={() => onEdit({ kind: "reminder", original: item })}
              >
                {uiText("edit")}
              </Button>
            )}
            <Button
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={() => setConfirming(true)}
            >
              {uiText("cancelReminder")}
            </Button>
          </>
        )
      }
    >
      <Line text={reminderLine(item, Date.now())} />
      {item.notes ? <Note text={item.notes} /> : null}
    </Row>
  );
}
