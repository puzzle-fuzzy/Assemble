import { Badge } from "@assemble/ui/badge";
import { Button } from "@assemble/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@assemble/ui/card";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@assemble/ui/controls/select";
import { DataTable, type DataTableColumn } from "@assemble/ui/data-table";
import { DatePicker } from "@assemble/ui/date-picker";
import { Empty, EmptyHeader, EmptyTitle } from "@assemble/ui/empty";
import { Field, FieldError, FieldGroup, FieldLabel } from "@assemble/ui/field";
import { Input } from "@assemble/ui/input";
import { parseCalendarDate } from "@assemble/ui/lib/date-input";
import { useId, useState } from "react";
import {
  type ActivityCategory,
  type ActivityPageData,
  type ActivitySnapshot,
  type ActivityTargetRef,
  type AdminActivity,
  type AdminActivityTarget,
  activityPath,
  subjectHref,
} from "../lib/activity-api";
import { useActivityTranslation } from "../lib/activity-i18n";
import type { AccessFailure } from "../lib/api";
import { RecordedTime } from "../lib/text";
import { useResource } from "../lib/use-resource";
import { DetailFields } from "./detail-dialog";
import { LoadingContent, RequestError } from "./feedback";
import { storageBytes } from "./storage-overview";

interface AccessProps {
  onAccessFailure: (failure: AccessFailure) => void;
}
const categories: ActivityCategory[] = [
  "all",
  "remote",
  "files",
  "media",
  "transfers",
];
const defaults = {
  q: "",
  category: "all" as ActivityCategory,
  from: "",
  through: "",
  page: 1,
  pageSize: 20,
};

function Actor({ entry }: { entry: AdminActivity }) {
  const { t } = useActivityTranslation();
  const automatic =
    entry.action === "send.expired" ||
    (entry.action === "remote.ended" &&
      entry.snapshot?.kind === "remote" &&
      ["expired", "disconnected", "deviceRevoked"].includes(
        entry.snapshot.endReason ?? "",
      ));
  return (
    <span title={entry.actor.accountId ?? undefined}>
      {entry.actor.accountId
        ? (entry.actor.name ?? t("unknownUser"))
        : automatic
          ? t("system")
          : t("unknown")}
    </span>
  );
}

function useActivityColumns(): DataTableColumn<AdminActivity>[] {
  const { t } = useActivityTranslation();
  return [
    {
      id: "time",
      label: t("time"),
      hideable: false,
      className: "min-w-44",
      cell: (entry) => <RecordedTime value={entry.occurredAt} />,
    },
    {
      id: "actor",
      label: t("actor"),
      className: "min-w-28 max-w-48 break-words",
      cell: (entry) => <Actor entry={entry} />,
    },
    {
      id: "device",
      label: t("device"),
      className: "min-w-28 max-w-48 break-words",
      cell: (entry) => entry.actor.deviceName ?? t("unknown"),
    },
    {
      id: "action",
      label: t("action"),
      hideable: false,
      className: "min-w-36",
      cell: (entry) =>
        t(`actions.${entry.action}`, { defaultValue: t("actions.other") }),
    },
    {
      id: "subject",
      label: t("subject"),
      hideable: false,
      className: "min-w-52 max-w-96 break-words whitespace-normal",
      cell: (entry) =>
        entry.subject ? (
          <Button
            asChild
            variant="link"
            className="h-auto max-w-full justify-start whitespace-normal p-0 text-left"
          >
            <a href={subjectHref(entry.subject)}>
              {entry.subject.name ?? t(`kinds.${entry.subject.kind}`)}
            </a>
          </Button>
        ) : (
          t("unknown")
        ),
    },
  ];
}

export function ActivityPage({
  target,
  onAccessFailure,
}: AccessProps & { target: ActivityTargetRef | null }) {
  const { t } = useActivityTranslation();
  const prefix = useId();
  const [draft, setDraft] = useState(defaults);
  const [filters, setFilters] = useState(defaults);
  const [invalid, setInvalid] = useState(false);
  const { resource, retry } = useResource<ActivityPageData>(
    activityPath(filters),
    onAccessFailure,
  );
  const page = resource.phase === "ready" ? resource.value : null;
  const tableColumns = useActivityColumns();
  const change = (field: "q" | "from" | "through", value: string) => {
    setDraft((previous) => ({ ...previous, [field]: value }));
    setInvalid(false);
  };
  return (
    <>
      <div hidden={target !== null}>
        <DataTable
          variant="management"
          label={t("title")}
          data={page?.items ?? []}
          getRowLabel={(entry) => String(entry.id)}
          columns={tableColumns}
          toolbar={
            <form
              className="flex min-w-0 flex-1 flex-wrap items-end gap-2"
              aria-label={t("search")}
              onSubmit={(event) => {
                event.preventDefault();
                if (
                  (draft.from && !parseCalendarDate(draft.from)) ||
                  (draft.through && !parseCalendarDate(draft.through)) ||
                  (draft.from && draft.through && draft.from > draft.through)
                ) {
                  setInvalid(true);
                  return;
                }
                setInvalid(false);
                setFilters({
                  ...draft,
                  q: draft.q.trim(),
                  page: 1,
                  pageSize: filters.pageSize,
                });
              }}
            >
              <FieldGroup className="flex-row flex-wrap items-end gap-2">
                <Field className="w-full sm:w-60">
                  <FieldLabel htmlFor={`${prefix}-q`} className="sr-only">
                    {t("search")}
                  </FieldLabel>
                  <Input
                    id={`${prefix}-q`}
                    type="search"
                    maxLength={200}
                    placeholder={t("searchPlaceholder")}
                    value={draft.q}
                    onChange={(event) => change("q", event.target.value)}
                  />
                </Field>
                <Field className="w-40">
                  <FieldLabel
                    className="sr-only"
                    htmlFor={`${prefix}-category`}
                  >
                    {t("category")}
                  </FieldLabel>
                  <Select
                    value={draft.category}
                    onValueChange={(value) =>
                      setDraft((previous) => ({
                        ...previous,
                        category: value as ActivityCategory,
                      }))
                    }
                  >
                    <SelectTrigger
                      id={`${prefix}-category`}
                      aria-label={t("category")}
                    >
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {categories.map((category) => (
                          <SelectItem key={category} value={category}>
                            {t(`categories.${category}`)}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </Field>
                <Field className="w-44" data-invalid={invalid}>
                  <FieldLabel className="sr-only" htmlFor={`${prefix}-from`}>
                    {t("from")}
                  </FieldLabel>
                  <DatePicker
                    id={`${prefix}-from`}
                    label={t("from")}
                    value={draft.from}
                    onChange={(value) => change("from", value)}
                  />
                </Field>
                <Field className="w-44" data-invalid={invalid}>
                  <FieldLabel className="sr-only" htmlFor={`${prefix}-through`}>
                    {t("through")}
                  </FieldLabel>
                  <DatePicker
                    id={`${prefix}-through`}
                    label={t("through")}
                    value={draft.through}
                    onChange={(value) => change("through", value)}
                  />
                </Field>
              </FieldGroup>
              <Button type="submit" variant="outline">
                {t("apply")}
              </Button>
              <Button
                type="button"
                variant="ghost"
                onClick={() => {
                  setDraft(defaults);
                  setFilters(defaults);
                  setInvalid(false);
                }}
              >
                {t("reset")}
              </Button>
              {invalid ? <FieldError>{t("invalidDates")}</FieldError> : null}
            </form>
          }
          actions={
            <Button
              variant="outline"
              disabled={resource.phase === "loading"}
              onClick={retry}
            >
              {t("refresh")}
            </Button>
          }
          pagination={{
            page: page?.page ?? filters.page,
            pageSize: filters.pageSize,
            total: page?.total ?? 0,
            pending: resource.phase === "loading",
            onChange: (page, pageSize) =>
              setFilters((previous) => ({ ...previous, page, pageSize })),
          }}
          rowActions={(entry) =>
            entry.subject
              ? [
                  {
                    id: "record",
                    label: t("view"),
                    href: `#activity/${entry.subject.kind}/${encodeURIComponent(entry.subject.id)}`,
                  },
                ]
              : []
          }
          emptyState={
            resource.phase === "loading" ? (
              <LoadingContent />
            ) : resource.phase === "error" ? (
              <RequestError failure={resource.failure} retry={retry} />
            ) : (
              <Empty>
                <EmptyHeader>
                  <EmptyTitle>
                    {t(
                      filters.q ||
                        filters.category !== "all" ||
                        filters.from ||
                        filters.through
                        ? "noMatches"
                        : "empty",
                    )}
                  </EmptyTitle>
                </EmptyHeader>
              </Empty>
            )
          }
        />
      </div>
      {target ? (
        <ActivityTargetPage
          key={`${target.kind}:${target.id}`}
          target={target}
          onAccessFailure={onAccessFailure}
        />
      ) : null}
    </>
  );
}

function SnapshotFields({ snapshot }: { snapshot: ActivitySnapshot }) {
  const { t, i18n } = useActivityTranslation();
  const fields: { label: string; value: string }[] = [];
  const field = (label: string, value: string | null) =>
    fields.push({ label: t(label), value: value ?? t("unknown") });
  switch (snapshot.kind) {
    case "file":
      field("name", snapshot.name);
      field(
        "size",
        storageBytes(snapshot.size, i18n.resolvedLanguage || i18n.language),
      );
      field("mime", snapshot.mimeType);
      break;
    case "transfer":
      field("name", snapshot.name);
      field(
        "size",
        storageBytes(snapshot.size, i18n.resolvedLanguage || i18n.language),
      );
      field("fromDevice", snapshot.fromDeviceName);
      field("toDevice", snapshot.toDeviceName);
      field(
        "status",
        t(`states.${snapshot.state}`, { defaultValue: t("unknown") }),
      );
      break;
    case "media":
      field("name", snapshot.title);
      field("scope", t(snapshot.scope));
      break;
    case "collection":
      field("name", snapshot.title);
      break;
    case "remote":
      field("controller", snapshot.controllerName);
      field("host", snapshot.hostName);
      field(
        "status",
        t(`states.${snapshot.phase}`, { defaultValue: t("unknown") }),
      );
      if (snapshot.endReason)
        field(
          "reason",
          t(`reasons.${snapshot.endReason}`, { defaultValue: t("unknown") }),
        );
      break;
  }
  return <DetailFields fields={fields} />;
}

function ActivityTargetPage({
  target,
  onAccessFailure,
}: AccessProps & { target: ActivityTargetRef }) {
  const { t } = useActivityTranslation();
  const { resource, retry } = useResource<AdminActivityTarget>(
    `/v1/admin/activity/targets/${target.kind}/${encodeURIComponent(target.id)}`,
    onAccessFailure,
  );
  const tableColumns = useActivityColumns();
  return (
    <div className="flex flex-col gap-4">
      <div>
        <Button asChild variant="outline">
          <a href="#activity">{t("back")}</a>
        </Button>
      </div>
      {resource.phase === "loading" ? (
        <LoadingContent />
      ) : resource.phase === "error" ? (
        <RequestError failure={resource.failure} retry={retry} />
      ) : (
        <>
          <Card>
            <CardHeader>
              <CardTitle>{t(`kinds.${target.kind}`)}</CardTitle>
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              {!resource.value.subject.available ? (
                <Badge variant="secondary">{t("deleted")}</Badge>
              ) : null}
              {resource.value.snapshot ? (
                <SnapshotFields snapshot={resource.value.snapshot} />
              ) : (
                <p className="text-sm text-muted-foreground">
                  {t("noSnapshot")}
                </p>
              )}
              <DetailFields
                fields={[
                  { label: t("id"), value: target.id },
                  {
                    label: t("owner"),
                    value:
                      resource.value.subject.ownerId ??
                      (resource.value.subject.kind === "collection" ||
                      (resource.value.snapshot?.kind === "media" &&
                        resource.value.snapshot.scope === "shared")
                        ? t("shared")
                        : t("unknown")),
                  },
                  {
                    label: t(
                      resource.value.snapshot ? "created" : "firstRecorded",
                    ),
                    value: <RecordedTime value={resource.value.createdAt} />,
                  },
                  {
                    label: t(
                      resource.value.snapshot ? "updated" : "lastRecorded",
                    ),
                    value: <RecordedTime value={resource.value.updatedAt} />,
                  },
                ]}
              />
              {resource.value.subject.available &&
              ["file", "media", "collection"].includes(target.kind) ? (
                <div>
                  <Button asChild variant="outline">
                    <a href={subjectHref(resource.value.subject)}>
                      {t("subject")}
                    </a>
                  </Button>
                </div>
              ) : null}
            </CardContent>
          </Card>
          <DataTable
            variant="management"
            label={t("history")}
            data={resource.value.events}
            getRowLabel={(entry) => String(entry.id)}
            columns={tableColumns}
            emptyState={
              <Empty>
                <EmptyHeader>
                  <EmptyTitle>{t("empty")}</EmptyTitle>
                </EmptyHeader>
              </Empty>
            }
          />
        </>
      )}
    </div>
  );
}
