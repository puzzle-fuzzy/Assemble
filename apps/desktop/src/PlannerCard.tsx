// 日程卡片：到点还没读的提醒、七天之内的安排，以及新建与修改的表单。
//
// 界面只摆 Rust 算好的东西：重复的日程展开成哪几场，在 Rust 那边定好。
// 每一次改动都带着看到的版本号；别处改过时 Rust 已经换上最新的名单，这里只提示"再看一眼"。
// 表单交给 Rust 的只有标题、时间与几句备注，没有这台电脑上的任何位置。

import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@/components/ui/empty";
import { type Bridge, toBridgeError } from "@/lib/bridge";
import { messageFor } from "@/lib/messages";
import { agenda } from "@/lib/planner-text";
import type {
  ReminderContent,
  ReminderView,
  ScheduleContent,
  ScheduleView,
  Snapshot,
} from "@/lib/types";
import { type Draft, type Form, PlannerForm } from "@/PlannerForm";
import {
  type Act,
  NotificationRow,
  ReminderRow,
  ScheduleRow,
} from "@/PlannerRows";

export interface PlannerCardProps {
  source: Bridge;
  snapshot: Snapshot | null;
  onError: (message: string | null) => void;
  active?: boolean;
  query?: string;
}

/** 这些错误码的意思都是"你看到的是旧的"：表单里留着旧值只会再错一次，不如收起来看最新的。 */
const STALE = new Set(["VERSION_CONFLICT", "INVALID_STATE"]);

export function PlannerCard({
  source,
  snapshot,
  onError,
  active = true,
  query = "",
}: PlannerCardProps) {
  const [pending, setPending] = useState(false);
  const [form, setForm] = useState<Form | null>(null);
  const [showForm, setShowForm] = useState(false);
  function edit(next: Form) {
    setForm(next);
    setShowForm(true);
  }
  // 名单由 Rust 在连上服务时去拿；万一没拿到（比如刚登录时断了一下），这里补要一次。
  const requested = useRef<string | null>(null);
  const signedInAs =
    snapshot?.session.phase === "signedIn" ? snapshot.session.device.id : null;
  const planner = snapshot?.planner ?? null;

  useEffect(() => {
    if (!signedInAs || planner || requested.current === signedInAs) return;
    requested.current = signedInAs;
    source.plannerRefresh().catch((error: unknown) => {
      onError(messageFor(toBridgeError(error)));
    });
  }, [signedInAs, planner, source, onError]);

  // 从托盘的"新建提醒…"进来：主窗口打开，直接摆出那张表单。
  const compose = planner?.compose ?? null;
  useEffect(() => {
    if (compose !== null) {
      setForm({ kind: compose, original: null });
      setShowForm(true);
    }
  }, [compose]);

  // 点开了一条系统通知：主窗口打开，滚到那一条并标出来。
  const focused = useRef<HTMLLIElement | null>(null);
  const focus = planner?.focus ?? null;
  useEffect(() => {
    if (focus !== null) setShowForm(false);
  }, [focus]);
  useEffect(() => {
    if (focus === null || !active || showForm) return;
    const row = focused.current;
    if (row && typeof row.scrollIntoView === "function")
      row.scrollIntoView({ block: "center" });
  }, [focus, active, showForm]);

  if (snapshot == null || snapshot.session.phase !== "signedIn") return null;
  const deviceName = snapshot.session.device.name;

  /** 做一件事；返回稳定错误码，成功时为 null。 */
  const act: Act = async (what) => {
    setPending(true);
    onError(null);
    try {
      await what();
      return null;
    } catch (error: unknown) {
      onError(messageFor(toBridgeError(error)));
      return toBridgeError(error).code;
    } finally {
      setPending(false);
    }
  };

  // 收起表单。它是从托盘里摆出来的，就顺便告诉 Rust 不用再摆了。
  async function close() {
    const fromTray = compose !== null;
    setForm(null);
    setShowForm(false);
    if (fromTray) await act(() => source.plannerCompose(null));
  }

  function apply(form: Form, draft: Draft): Promise<unknown> {
    if (form.kind === "schedule") {
      const original = form.original;
      const content: ScheduleContent = {
        title: draft.title,
        startAt: draft.start,
        endAt: draft.end,
        repeat: draft.repeat,
        // 不重复的日程没有截止日；重复的沿用原来的，表单里不改它。
        repeatUntil:
          draft.repeat === "none" ? null : (original?.repeatUntil ?? null),
        remindBefore: draft.remindBefore,
        notes: draft.notes,
        // 地点没改就原样留着（导航用的地址还在）；改了就只剩新写的那一个名字。
        location:
          draft.place === ""
            ? null
            : original?.location?.label === draft.place
              ? original.location
              : { label: draft.place, address: draft.place },
      };
      return original === null
        ? source.scheduleCreate(content)
        : source.scheduleUpdate(original.id, original.version, content);
    }
    const original = form.original;
    const content: ReminderContent = {
      title: draft.title,
      dueAt: draft.start,
      notes: draft.notes,
    };
    return original === null
      ? source.reminderCreate(content)
      : source.reminderUpdate(original.id, original.version, content);
  }

  /** 存好了，或者看到的已经是旧的，就收起表单。 */
  async function save(form: Form, draft: Draft) {
    const code = await act(() => apply(form, draft));
    if (code === null || STALE.has(code)) await close();
  }

  async function removeIt(original: ScheduleView | ReminderView) {
    const code = await act(() =>
      "startAt" in original
        ? source.scheduleDelete(original.id, original.version)
        : source.reminderDelete(original.id, original.version),
    );
    if (code === null || STALE.has(code)) await close();
  }

  const entries = planner ? agenda(planner, Date.now()) : [];
  const original = form?.original ?? null;
  const needle = query.trim().toLocaleLowerCase();
  const notifications =
    planner?.notifications.filter((item) =>
      `${item.title} ${item.body ?? ""}`.toLocaleLowerCase().includes(needle),
    ) ?? [];
  const shownEntries = entries.filter((entry) => {
    const item = "schedule" in entry ? entry.schedule : entry.reminder;
    return `${item.title} ${item.notes ?? ""} ${"location" in item ? (item.location?.label ?? "") : ""}`
      .toLocaleLowerCase()
      .includes(needle);
  });

  return (
    <Card>
      <CardHeader>
        <CardTitle>日程</CardTitle>
        {showForm || (planner !== null && entries.length === 0) ? null : (
          <CardDescription>
            {planner ? `七天之内有 ${entries.length} 项。` : "正在读取日程…"}
          </CardDescription>
        )}
        {showForm ? null : (
          <CardAction className="flex flex-wrap gap-2">
            {form ? (
              <Button
                size="sm"
                variant="outline"
                disabled={pending}
                onClick={() => setShowForm(true)}
              >
                继续编辑…
              </Button>
            ) : null}
            <Button
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={() => edit({ kind: "schedule", original: null })}
            >
              新建日程…
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={() => edit({ kind: "reminder", original: null })}
            >
              新建提醒…
            </Button>
          </CardAction>
        )}
      </CardHeader>
      {form ? (
        <CardContent hidden={!showForm}>
          <PlannerForm
            // 换一张表单就是换一份草稿：重新挂一个，上一张里写的不会带过来。
            key={`${form.kind}:${original?.id ?? "new"}`}
            form={form}
            pending={pending}
            onSave={(draft) => save(form, draft)}
            onClose={() => void close()}
            onRemove={original ? () => removeIt(original) : null}
          />
        </CardContent>
      ) : null}
      {!showForm && planner ? (
        <CardContent className="flex flex-col gap-4">
          {notifications.length > 0 ? (
            <div className="flex flex-col gap-2">
              <div className="flex items-center justify-between gap-3">
                <h2 className="text-sm font-medium">到点的提醒</h2>
                {planner.notifications.length > 1 ? (
                  <Button
                    size="xs"
                    variant="ghost"
                    disabled={pending}
                    onClick={() =>
                      void act(() => source.notificationsReadAll())
                    }
                  >
                    全部已读
                  </Button>
                ) : null}
              </div>
              <ul className="flex flex-col gap-2">
                {notifications.map((notification) => (
                  <NotificationRow
                    key={notification.id}
                    ref={notification.id === focus ? focused : undefined}
                    notification={notification}
                    reminder={
                      planner.reminders.find(
                        (candidate) => candidate.id === notification.reminderId,
                      ) ?? null
                    }
                    highlighted={notification.id === focus}
                    pending={pending}
                    onRead={() =>
                      void act(() =>
                        source.notificationsRead([notification.id]),
                      )
                    }
                    onDone={(reminder) =>
                      void act(() =>
                        source.reminderDismiss(reminder.id, reminder.version),
                      )
                    }
                  />
                ))}
              </ul>
            </div>
          ) : null}
          <div className="flex flex-col gap-2">
            <h2 className="text-sm font-medium">接下来的安排</h2>
            {entries.length === 0 ? (
              <Empty>
                <EmptyHeader>
                  <EmptyTitle>七天之内没有安排。</EmptyTitle>
                  <EmptyDescription>
                    新建日程或提醒后会显示在这里。
                  </EmptyDescription>
                </EmptyHeader>
              </Empty>
            ) : shownEntries.length === 0 && notifications.length === 0 ? (
              <Empty>
                <EmptyHeader>
                  <EmptyTitle>没有匹配的安排</EmptyTitle>
                  <EmptyDescription>换个关键词试试。</EmptyDescription>
                </EmptyHeader>
              </Empty>
            ) : (
              <ul className="flex flex-col gap-2">
                {shownEntries.map((entry) =>
                  "schedule" in entry ? (
                    <ScheduleRow
                      key={entry.key}
                      source={source}
                      item={entry.schedule}
                      deviceName={deviceName}
                      pending={pending}
                      act={act}
                      onEdit={edit}
                    />
                  ) : (
                    <ReminderRow
                      key={entry.key}
                      source={source}
                      item={entry.reminder}
                      deviceName={deviceName}
                      pending={pending}
                      act={act}
                      onEdit={edit}
                    />
                  ),
                )}
              </ul>
            )}
          </div>
        </CardContent>
      ) : null}
    </Card>
  );
}
