import { Alert, AlertDescription } from "@assemble/ui/alert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@assemble/ui/alert-dialog";
import { Button } from "@assemble/ui/button";
import { DataTable } from "@assemble/ui/data-table";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@assemble/ui/dialog";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@assemble/ui/empty";
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
} from "@assemble/ui/field";
import { Input } from "@assemble/ui/input";
import { LoadingState } from "@assemble/ui/loading-state";
import { Textarea } from "@assemble/ui/textarea";
import { type ReactNode, useEffect, useId, useRef, useState } from "react";
import { toast } from "sonner";
import { LocatedText } from "@/LocatedText";
import { toBridgeError } from "@/lib/bridge";
import { useMemoTranslation } from "@/lib/memo-i18n";
import {
  type Memo,
  type MemoBridge,
  type MemoContent,
  type MemoList,
  type MemoSummary,
  validMemoContent,
} from "@/lib/memos";

export function MemosPage({
  source,
  accountId,
  onSignIn,
}: {
  source: MemoBridge;
  accountId: string | null;
  onSignIn: () => void;
}) {
  const { t } = useMemoTranslation();
  if (accountId === null)
    return (
      <Empty>
        <EmptyHeader>
          <EmptyTitle>{t("signIn")}</EmptyTitle>
          <EmptyDescription>{t("privateHint")}</EmptyDescription>
        </EmptyHeader>
        <EmptyContent>
          <Button variant="outline" onClick={onSignIn}>
            {t("openAccount")}
          </Button>
        </EmptyContent>
      </Empty>
    );
  return (
    <MemosContent key={accountId} source={source} onAccessFailure={onSignIn} />
  );
}

type Editor = {
  memo: Memo | null;
  requestId: string;
  title: string;
  text: string;
};
function MemosContent({
  source,
  onAccessFailure,
}: {
  source: MemoBridge;
  onAccessFailure: () => void;
}) {
  const { t, i18n } = useMemoTranslation();
  const prefix = useId();
  const [query, setQuery] = useState({ q: "", page: 1, pageSize: 10 });
  const [draft, setDraft] = useState("");
  const [list, setList] = useState<MemoList | null>(null);
  const [revision, setRevision] = useState(0);
  const [pending, setPending] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [listError, setListError] = useState(false);
  const [editor, setEditor] = useState<Editor | null>(null);
  const [details, setDetails] = useState<Memo | null>(null);
  const [removing, setRemoving] = useState<MemoSummary | null>(null);
  const trigger = useRef<HTMLElement | null>(null);
  const createTrigger = useRef<HTMLButtonElement>(null);
  const accessFailure = useRef(onAccessFailure);
  useEffect(() => {
    accessFailure.current = onAccessFailure;
  }, [onAccessFailure]);
  const tasks = useRef(new Set<AbortController>());
  const mutationRunning = useRef(false);
  useEffect(
    () => () => {
      for (const task of tasks.current) task.abort();
    },
    [],
  );
  useEffect(() => {
    void revision;
    const controller = new AbortController();
    setPending(true);
    setList(null);
    setListError(false);
    source
      .list(query, controller.signal)
      .then((value) => {
        if (!controller.signal.aborted) {
          const last = Math.max(1, Math.ceil(value.total / query.pageSize));
          if (query.page > last)
            setQuery((previous) => ({ ...previous, page: last }));
          else setList(value);
        }
      })
      .catch((failure) => {
        if (!controller.signal.aborted) {
          if (toBridgeError(failure).code === "UNAUTHORIZED")
            accessFailure.current();
          else setListError(true);
        }
      })
      .finally(() => {
        if (!controller.signal.aborted) setPending(false);
      });
    return () => controller.abort();
  }, [source, query, revision]);
  function captureFocus() {
    trigger.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
  }
  function restoreFocus(event: Event) {
    event.preventDefault();
    if (trigger.current?.isConnected) trigger.current.focus();
    else createTrigger.current?.focus();
  }
  async function action<T>(
    operation: (signal: AbortSignal) => Promise<T>,
    done: (value: T) => void,
    copy = false,
  ) {
    if (!copy && mutationRunning.current) return;
    if (!copy) {
      mutationRunning.current = true;
      setBusy(true);
    }
    const controller = new AbortController();
    tasks.current.add(controller);
    setError(null);
    try {
      const value = await operation(controller.signal);
      if (!controller.signal.aborted) done(value);
    } catch (failure) {
      if (!controller.signal.aborted) {
        const code = toBridgeError(failure).code;
        if (code === "UNAUTHORIZED") onAccessFailure();
        else
          setError(
            code === "VERSION_CONFLICT"
              ? "conflict"
              : code === "NOT_FOUND"
                ? "notFound"
                : copy
                  ? "copyFailed"
                  : "failed",
          );
      }
    } finally {
      tasks.current.delete(controller);
      if (!copy) {
        mutationRunning.current = false;
        if (!controller.signal.aborted) setBusy(false);
      }
    }
  }
  function open(memo: MemoSummary, editing: boolean) {
    captureFocus();
    setError(null);
    void action(
      (signal) => source.get(memo.id, signal),
      (value) => {
        if (editing)
          setEditor({
            memo: value,
            requestId: crypto.randomUUID(),
            title: value.title,
            text: value.text,
          });
        else setDetails(value);
      },
    );
  }
  function copy(id: string) {
    void action(
      (signal) => source.copy(id, signal),
      () => toast.success(t("copied")),
      true,
    );
  }
  function save() {
    if (!editor) return;
    const content: MemoContent = { title: editor.title, text: editor.text };
    if (!validMemoContent(content)) {
      setError("invalid");
      return;
    }
    void action(
      (signal) =>
        editor.memo
          ? source.update(editor.memo.id, editor.memo.version, content, signal)
          : source.create(content, editor.requestId, signal),
      () => {
        setEditor(null);
        setRevision((value) => value + 1);
      },
    );
  }
  function problem(): ReactNode {
    return (
      error && (
        <Alert variant="destructive">
          <AlertDescription>{t(error)}</AlertDescription>
        </Alert>
      )
    );
  }
  const time = (value: string) => (
    <time dateTime={value}>
      {new Intl.DateTimeFormat(i18n.resolvedLanguage || i18n.language, {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(new Date(value))}
    </time>
  );
  return (
    <div className="flex min-w-0 flex-col gap-4">
      {!editor && !removing && !details && problem()}
      <DataTable
        variant="management"
        label={t("title")}
        data={list?.items ?? []}
        getRowLabel={(memo) => memo.title}
        columns={[
          {
            id: "title",
            label: t("memoTitle"),
            hideable: false,
            className: "min-w-36 max-w-72 whitespace-normal break-words",
            cell: (memo) => memo.title,
          },
          {
            id: "preview",
            label: t("preview"),
            className: "min-w-52 max-w-xl whitespace-normal",
            cell: (memo) => (
              <span className="line-clamp-2 whitespace-pre-wrap break-all">
                {memo.preview}
              </span>
            ),
          },
          {
            id: "updatedAt",
            label: t("updatedAt"),
            cell: (memo) => time(memo.updatedAt),
          },
        ]}
        toolbar={
          <form
            className="w-full max-w-80"
            aria-label={t("searchLabel")}
            onSubmit={(event) => {
              event.preventDefault();
              setQuery((previous) => ({
                ...previous,
                q: draft.trim(),
                page: 1,
              }));
            }}
          >
            <FieldGroup>
              <Field>
                <FieldLabel className="sr-only" htmlFor={`${prefix}-search`}>
                  {t("searchLabel")}
                </FieldLabel>
                <div className="flex items-center gap-2">
                  <Input
                    id={`${prefix}-search`}
                    type="search"
                    maxLength={200}
                    value={draft}
                    placeholder={t("searchPlaceholder")}
                    onChange={(event) => setDraft(event.target.value)}
                  />
                  <Button variant="outline" size="default" type="submit">
                    {t("search")}
                  </Button>
                </div>
              </Field>
            </FieldGroup>
          </form>
        }
        actions={
          <>
            <Button
              size="default"
              variant="outline"
              disabled={pending}
              onClick={() => setRevision((value) => value + 1)}
            >
              {t("refresh")}
            </Button>
            <Button
              ref={createTrigger}
              size="default"
              disabled={busy}
              onClick={() => {
                captureFocus();
                setError(null);
                setEditor({
                  memo: null,
                  requestId: crypto.randomUUID(),
                  title: "",
                  text: "",
                });
              }}
            >
              {t("create")}
            </Button>
          </>
        }
        pagination={{
          page: list?.page ?? query.page,
          pageSize: query.pageSize,
          total: list?.total ?? 0,
          pending,
          onChange: (page, pageSize) =>
            setQuery((previous) => ({ ...previous, page, pageSize })),
        }}
        rowActions={(memo) => [
          { id: "copy", label: t("copy"), onSelect: () => copy(memo.id) },
          {
            id: "details",
            label: t("details"),
            disabled: busy,
            onSelect: () => open(memo, false),
          },
          {
            id: "edit",
            label: t("edit"),
            disabled: busy,
            onSelect: () => open(memo, true),
          },
          {
            id: "remove",
            label: t("remove"),
            destructive: true,
            disabled: busy,
            onSelect: () => {
              captureFocus();
              setError(null);
              setRemoving(memo);
            },
          },
        ]}
        emptyState={
          pending ? (
            <LoadingState className="h-24 w-full" label={t("loading")} />
          ) : listError ? (
            <Alert>
              <AlertDescription className="flex flex-wrap items-center gap-3">
                {t("error")}
                <Button
                  variant="outline"
                  size="default"
                  onClick={() => setRevision((value) => value + 1)}
                >
                  {t("retry")}
                </Button>
              </AlertDescription>
            </Alert>
          ) : (
            <Empty>
              <EmptyHeader>
                <EmptyTitle>{t(query.q ? "noMatches" : "empty")}</EmptyTitle>
                <EmptyDescription>
                  {t(query.q ? "noMatchesHint" : "emptyHint")}
                </EmptyDescription>
              </EmptyHeader>
            </Empty>
          )
        }
      />
      <Dialog
        open={editor !== null}
        onOpenChange={(value) => {
          if (!value && !busy) {
            setEditor(null);
            setError(null);
          }
        }}
      >
        <DialogContent
          className="max-h-[85svh] overflow-y-auto sm:max-w-2xl"
          onCloseAutoFocus={restoreFocus}
          showCloseButton={!busy}
        >
          <DialogHeader>
            <DialogTitle>{t(editor?.memo ? "edit" : "create")}</DialogTitle>
            <DialogDescription>{t("textHint")}</DialogDescription>
          </DialogHeader>
          {problem()}
          <form
            onSubmit={(event) => {
              event.preventDefault();
              save();
            }}
            className="flex flex-col gap-4"
          >
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor={`${prefix}-title`}>
                  {t("memoTitle")}
                </FieldLabel>
                <Input
                  id={`${prefix}-title`}
                  required
                  maxLength={200}
                  value={editor?.title ?? ""}
                  placeholder={t("titlePlaceholder")}
                  disabled={busy}
                  onChange={(event) =>
                    setEditor(
                      (previous) =>
                        previous && {
                          ...previous,
                          title: event.target.value,
                          requestId: crypto.randomUUID(),
                        },
                    )
                  }
                />
              </Field>
              <Field>
                <FieldLabel htmlFor={`${prefix}-text`}>{t("text")}</FieldLabel>
                <Textarea
                  id={`${prefix}-text`}
                  required
                  maxLength={100_000}
                  rows={12}
                  className="min-h-52 whitespace-pre-wrap"
                  value={editor?.text ?? ""}
                  placeholder={t("textPlaceholder")}
                  disabled={busy}
                  onChange={(event) =>
                    setEditor(
                      (previous) =>
                        previous && {
                          ...previous,
                          text: event.target.value,
                          requestId: crypto.randomUUID(),
                        },
                    )
                  }
                />
                <FieldDescription>
                  {(editor?.text.length ?? 0).toLocaleString(
                    i18n.resolvedLanguage || i18n.language,
                  )}{" "}
                  / 100,000
                </FieldDescription>
              </Field>
            </FieldGroup>
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                disabled={busy}
                onClick={() => {
                  setEditor(null);
                  setError(null);
                }}
              >
                {t("cancel")}
              </Button>
              <Button type="submit" disabled={busy}>
                {t(busy ? "saving" : "save")}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
      <Dialog
        open={details !== null}
        onOpenChange={(value) => {
          if (!value) setDetails(null);
        }}
      >
        <DialogContent
          className="max-h-[85svh] overflow-y-auto sm:max-w-2xl"
          onCloseAutoFocus={restoreFocus}
        >
          <DialogHeader>
            <DialogTitle>{details?.title}</DialogTitle>
            <DialogDescription>
              {details && time(details.updatedAt)}
            </DialogDescription>
          </DialogHeader>
          {problem()}
          <FieldGroup>
            <Field>
              {details?.locations?.length && source.openLocation ? (
                <>
                  <FieldLabel asChild>
                    <span>{t("text")}</span>
                  </FieldLabel>
                  <div className="min-h-52 whitespace-pre-wrap break-words text-sm">
                    <LocatedText
                      text={details.text}
                      locations={details.locations}
                      onOpen={(label) => {
                        void source
                          .openLocation?.(details.id, label)
                          .catch((error) => {
                            const code = toBridgeError(error).code;
                            setError(code);
                          });
                      }}
                    />
                  </div>
                </>
              ) : (
                <>
                  <FieldLabel htmlFor={`${prefix}-read`}>
                    {t("text")}
                  </FieldLabel>
                  <Textarea
                    id={`${prefix}-read`}
                    readOnly
                    rows={12}
                    className="min-h-52 whitespace-pre-wrap"
                    value={details?.text ?? ""}
                  />
                </>
              )}
            </Field>
          </FieldGroup>
          <DialogFooter>
            <Button variant="outline" onClick={() => setDetails(null)}>
              {t("close")}
            </Button>
            <Button
              onClick={() => {
                if (details) copy(details.id);
              }}
            >
              {t("copy")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <AlertDialog
        open={removing !== null}
        onOpenChange={(value) => {
          if (!value && !busy) {
            setRemoving(null);
            setError(null);
          }
        }}
      >
        <AlertDialogContent onCloseAutoFocus={restoreFocus}>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("removeTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {removing?.title} — {t("removeDescription")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          {problem()}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={busy}>{t("cancel")}</AlertDialogCancel>
            <AlertDialogAction
              disabled={busy}
              onClick={(event) => {
                event.preventDefault();
                if (removing)
                  void action(
                    (signal) =>
                      source.remove(removing.id, removing.version, signal),
                    () => {
                      setRemoving(null);
                      setRevision((value) => value + 1);
                    },
                  );
              }}
            >
              {t("remove")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
