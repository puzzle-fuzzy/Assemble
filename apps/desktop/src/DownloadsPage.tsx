import { Alert, AlertDescription } from "@assemble/ui/alert";
import { Button } from "@assemble/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@assemble/ui/card";
import { Checkbox } from "@assemble/ui/checkbox";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@assemble/ui/controls/select";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@assemble/ui/dialog";
import { Empty, EmptyHeader, EmptyTitle } from "@assemble/ui/empty";
import {
  Field,
  FieldError,
  FieldGroup,
  FieldLabel as Label,
} from "@assemble/ui/field";
import { Input } from "@assemble/ui/input";
import { Progress } from "@assemble/ui/progress";
import { useEffect, useId, useRef, useState } from "react";
import { type Bridge, toBridgeError } from "@/lib/bridge";
import {
  downloadError,
  downloadText as t,
  useDownloadTranslation,
} from "@/lib/download-i18n";
import {
  type DownloadKind,
  type DownloadPreview,
  type DownloadsView,
  downloadProgress,
} from "@/lib/downloads";
import { formatSize } from "@/lib/transfer-text";

export function DownloadsPage({
  source,
  view,
}: {
  source: Bridge;
  view: DownloadsView | undefined;
}) {
  useDownloadTranslation();
  const id = useId();
  const locked = useRef(false);
  const [kind, setKind] = useState<DownloadKind>("hls");
  const [directBt, setDirectBt] = useState(false);
  const [url, setUrl] = useState("");
  const [page, setPage] = useState("");
  const [preview, setPreview] = useState<DownloadPreview | null>(null);
  const [fileIds, setFileIds] = useState<number[]>([]);
  const composeRef = useRef(false);
  const [variant, setVariant] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [compose, setCompose] = useState(false);
  const needsImportLink = error === "IMPORT_DOWNLOAD_LINK_NEEDED";
  useEffect(() => {
    if (compose && needsImportLink && !pending)
      document.getElementById(`${id}-url`)?.focus();
  }, [compose, id, needsImportLink, pending]);
  async function run(
    action: () => Promise<unknown>,
    invalidSourceError = "DOWNLOAD_INVALID_SOURCE",
  ) {
    if (locked.current) return;
    locked.current = true;
    setPending(true);
    setError(null);
    try {
      await action();
    } catch (cause) {
      const code = toBridgeError(cause).code;
      const message =
        code === "DOWNLOAD_INVALID_SOURCE" ? invalidSourceError : code;
      setError(message);
    } finally {
      locked.current = false;
      setPending(false);
    }
  }
  function accept(value: DownloadPreview | null) {
    if (value) {
      if (!composeRef.current) return;
      setPreview(value);
      setFileIds(value.files?.map((f) => f.id) ?? []);
      setVariant(value.variants[0]?.id ?? "");
      setUrl("");
      setPage("");
    }
  }
  return (
    <Dialog
      open={compose}
      onOpenChange={(open) => {
        composeRef.current = open;
        setCompose(open);
        if (!open && pending && kind === "magnet") {
          void source
            .downloadCancelPreparation()
            .catch((cause) => setError(toBridgeError(cause).code));
        }
      }}
    >
      <DialogContent
        aria-describedby={undefined}
        className="max-h-[85dvh] overflow-y-auto sm:max-w-xl"
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          document.getElementById(`${id}-url`)?.focus();
        }}
      >
        <DialogHeader>
          <DialogTitle>{t("new")}</DialogTitle>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void run(async () => {
              if (!url.trim()) {
                setError("DOWNLOAD_INVALID_SOURCE");
                return;
              }
              accept(
                await source.downloadPrepare({
                  kind,
                  ...(kind === "magnet" && directBt
                    ? { magnetNetwork: "direct" as const }
                    : {}),
                  url,
                  pageUrl: kind === "hls" ? page || null : null,
                }),
              );
            });
          }}
        >
          <FieldGroup>
            <Field>
              <Label htmlFor={`${id}-kind`}>{t("source")}</Label>
              <Select
                value={kind}
                disabled={pending}
                onValueChange={(value) => {
                  setKind(value as DownloadKind);
                  setPreview(null);
                  setError(null);
                }}
              >
                <SelectTrigger id={`${id}-kind`} className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent position="popper">
                  <SelectGroup>
                    <SelectItem value="hls">{t("hls")}</SelectItem>
                    <SelectItem value="douyin">{t("douyin")}</SelectItem>
                    <SelectItem value="magnet">{t("magnet")}</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
            <Field>
              <Label htmlFor={`${id}-url`}>
                {t(
                  kind === "hls"
                    ? "hlsAddress"
                    : kind === "magnet"
                      ? "magnetAddress"
                      : "address",
                )}
              </Label>
              <Input
                id={`${id}-url`}
                value={url}
                onChange={(e) => {
                  setUrl(e.target.value);
                  setPreview(null);
                  setError(null);
                }}
                aria-invalid={needsImportLink}
                aria-describedby={needsImportLink ? `${id}-error` : undefined}
                disabled={pending}
                autoComplete="off"
                spellCheck={false}
                maxLength={16384}
              />
            </Field>
            {kind === "hls" ? (
              <Field>
                <Label htmlFor={`${id}-page`}>{t("page")}</Label>
                <Input
                  id={`${id}-page`}
                  value={page}
                  onChange={(e) => {
                    setPage(e.target.value);
                    setPreview(null);
                    setError(null);
                  }}
                  disabled={pending}
                  autoComplete="off"
                  spellCheck={false}
                  maxLength={16384}
                />
              </Field>
            ) : null}
            {kind === "magnet" ? (
              <div className="space-y-2">
                <p className="text-sm text-muted-foreground">
                  {t("magnetHint")}
                </p>
                {view?.directBtAvailable ? (
                  <label
                    className="flex items-center gap-2 text-sm"
                    htmlFor={`${id}-direct-bt`}
                  >
                    <Checkbox
                      id={`${id}-direct-bt`}
                      checked={directBt}
                      disabled={pending}
                      onCheckedChange={(checked) => {
                        setDirectBt(checked === true);
                        setPreview(null);
                      }}
                    />
                    {t("directBt")}
                  </label>
                ) : null}
                {view?.directBtAvailable ? (
                  <p className="text-xs text-muted-foreground">
                    {t("directBtHint")}
                  </p>
                ) : null}
              </div>
            ) : null}
            <div className="flex flex-wrap gap-2">
              <Button type="submit" disabled={pending}>
                {t(pending ? "parsing" : "parse")}
              </Button>
              {pending && kind === "magnet" ? (
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => {
                    void source
                      .downloadCancelPreparation()
                      .catch((cause) => setError(toBridgeError(cause).code));
                  }}
                >
                  {t("cancelParse")}
                </Button>
              ) : null}
              {kind === "hls" ? (
                <Button
                  type="button"
                  variant="outline"
                  disabled={pending}
                  onClick={() =>
                    void run(
                      async () =>
                        accept(
                          await source.downloadImportHls(
                            url.trim() || null,
                            page || null,
                          ),
                        ),
                      url.trim() ? undefined : "IMPORT_DOWNLOAD_LINK_NEEDED",
                    )
                  }
                >
                  {t("import")}
                </Button>
              ) : null}
            </div>
          </FieldGroup>
        </form>
        {error ? (
          <FieldError id={`${id}-error`}>{downloadError(error)}</FieldError>
        ) : null}
        {preview ? (
          <Card size="sm">
            <CardHeader>
              <CardTitle className="break-words">{preview.title}</CardTitle>
              <CardDescription>
                {preview.sourceHost}
                {preview.kind === "magnet"
                  ? ` · ${t(preview.magnetNetwork === "direct" ? "directBt" : "systemNetwork")}`
                  : ""}
              </CardDescription>
            </CardHeader>
            <CardContent>
              {preview.kind === "magnet" ? (
                <div className="space-y-3">
                  <p className="text-sm">
                    {t("selectedFiles", {
                      count: fileIds.length,
                      size: formatSize(
                        (preview.files ?? [])
                          .filter((f) => fileIds.includes(f.id))
                          .reduce((n, f) => n + f.size, 0),
                      ),
                    })}
                  </p>
                  <div className="flex gap-2">
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      disabled={pending}
                      onClick={() =>
                        setFileIds(preview.files?.map((f) => f.id) ?? [])
                      }
                    >
                      {t("selectAll")}
                    </Button>
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      disabled={pending}
                      onClick={() => setFileIds([])}
                    >
                      {t("selectNone")}
                    </Button>
                  </div>
                  <ul
                    className="max-h-64 space-y-2 overflow-auto"
                    aria-label={t("files")}
                  >
                    {(preview.files ?? []).map((file) => (
                      <li key={file.id}>
                        <label
                          className="flex items-start gap-2 text-sm"
                          htmlFor={`${id}-file-${file.id}`}
                        >
                          <Checkbox
                            id={`${id}-file-${file.id}`}
                            disabled={pending}
                            checked={fileIds.includes(file.id)}
                            onCheckedChange={(checked) =>
                              setFileIds((current) =>
                                checked === true
                                  ? [...current, file.id]
                                  : current.filter((id) => id !== file.id),
                              )
                            }
                          />
                          <span className="min-w-0 flex-1 break-all">
                            {file.name}
                          </span>
                          <span className="shrink-0 text-muted-foreground">
                            {formatSize(file.size)}
                          </span>
                        </label>
                      </li>
                    ))}
                  </ul>
                </div>
              ) : (
                <FieldGroup>
                  <Field>
                    <Label htmlFor={`${id}-quality`}>{t("quality")}</Label>
                    <Select
                      value={variant}
                      disabled={pending}
                      onValueChange={setVariant}
                    >
                      <SelectTrigger id={`${id}-quality`} className="w-full">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent position="popper">
                        <SelectGroup>
                          {preview.variants.map((v) => (
                            <SelectItem key={v.id} value={v.id}>
                              {v.height ? `${v.height}p` : t("original")}
                              {v.bandwidth
                                ? ` · ${t("rate", { rate: Math.round(v.bandwidth / 1000) })}`
                                : ""}
                            </SelectItem>
                          ))}
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                  </Field>
                </FieldGroup>
              )}
            </CardContent>
            <CardFooter>
              <Button
                disabled={
                  pending ||
                  (preview.kind === "magnet"
                    ? fileIds.length === 0
                    : !view?.toolsAvailable)
                }
                onClick={() =>
                  void run(async () => {
                    await source.downloadStart(
                      preview.id,
                      variant,
                      preview.kind === "magnet" ? fileIds : [],
                    );
                    setPreview(null);
                    setDirectBt(false);
                    composeRef.current = false;
                    setCompose(false);
                  })
                }
              >
                {t("start")}
              </Button>
            </CardFooter>
          </Card>
        ) : null}
      </DialogContent>
      <Card>
        <CardHeader className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <CardTitle>{t("queue")}</CardTitle>
            <p className="mt-1 break-words text-sm text-muted-foreground">
              {t("folder", { name: view?.folderName || "网络下载" })}
            </p>
          </div>
          <CardAction className="ms-auto flex min-w-0 flex-wrap gap-2">
            <DialogTrigger asChild>
              <Button type="button" variant="outline" size="default">
                {t("new")}
              </Button>
            </DialogTrigger>
            <Button
              type="button"
              variant="outline"
              size="default"
              disabled={pending}
              onClick={() => void run(() => source.downloadChooseFolder())}
            >
              {t("chooseFolder")}
            </Button>
          </CardAction>
        </CardHeader>
        <CardContent className="space-y-4">
          {!view?.toolsAvailable ? (
            <p className="text-sm text-muted-foreground">{t("toolsMissing")}</p>
          ) : null}
          {error && !compose ? (
            <Alert variant="destructive">
              <AlertDescription>{downloadError(error)}</AlertDescription>
            </Alert>
          ) : null}
          {!view?.tasks.length ? (
            <Empty className="py-8">
              <EmptyHeader>
                <EmptyTitle>{t("empty")}</EmptyTitle>
              </EmptyHeader>
            </Empty>
          ) : (
            <ul className="divide-y">
              {view.tasks.map((task) => {
                const active = [
                  "queued",
                  "downloading",
                  "muxing",
                  "verifying",
                ].includes(task.phase);
                const progress = downloadProgress(task);
                return (
                  <li
                    key={task.id}
                    id={`network-download-${task.id}`}
                    tabIndex={-1}
                    className="flex flex-col gap-2 py-4 first:pt-0 last:pb-0"
                  >
                    <p className="break-words text-sm font-medium">
                      {task.fileName ?? task.title}
                    </p>
                    {task.phase !== "completed" ? (
                      <p
                        className="text-xs text-muted-foreground"
                        role={active ? undefined : "status"}
                      >
                        {t(task.phase)}
                      </p>
                    ) : null}
                    {active || task.phase === "paused" ? (
                      <Progress
                        value={progress}
                        aria-label={`${task.title} · ${t(task.phase)}`}
                      />
                    ) : null}
                    <div className="flex flex-wrap gap-2 text-xs text-muted-foreground">
                      {task.kind === "magnet" ? (
                        <span>
                          {t(
                            task.magnetNetwork === "direct"
                              ? "directBt"
                              : "systemNetwork",
                          )}
                        </span>
                      ) : null}
                      {task.initiatedBy === "assistant" ? (
                        <span>{t("initiatedByAssistant")}</span>
                      ) : null}
                      <span>
                        {formatSize(task.receivedBytes)}
                        {task.kind === "magnet" && task.totalBytes !== null
                          ? ` / ${formatSize(task.totalBytes)}`
                          : ""}
                      </span>
                      {active && task.totalSegments !== null ? (
                        <span>
                          {t("parts", {
                            done: task.completedSegments,
                            total: task.totalSegments,
                          })}
                        </span>
                      ) : null}
                      {active && task.bytesPerSecond > 0 ? (
                        <span>
                          {t("speed", {
                            size: formatSize(task.bytesPerSecond),
                          })}
                        </span>
                      ) : null}
                    </div>
                    {task.errorCode ? (
                      <p className="text-sm text-destructive">
                        {downloadError(task.errorCode)}
                      </p>
                    ) : null}
                    <div className="flex flex-wrap gap-2">
                      {active && task.kind === "magnet" ? (
                        <Button
                          type="button"
                          variant="outline"
                          disabled={pending}
                          onClick={() =>
                            void run(() => source.downloadPause(task.id))
                          }
                        >
                          {t("pause")}
                        </Button>
                      ) : null}
                      {task.kind === "magnet" && task.phase === "paused" ? (
                        <Button
                          type="button"
                          variant="outline"
                          disabled={pending}
                          onClick={() =>
                            void run(() => source.downloadResume(task.id))
                          }
                        >
                          {t("resume")}
                        </Button>
                      ) : null}
                      {active ||
                      (task.kind === "magnet" &&
                        ["paused", "failed", "interrupted"].includes(
                          task.phase,
                        )) ? (
                        <Button
                          type="button"
                          variant="outline"
                          size="default"
                          disabled={pending}
                          onClick={() =>
                            void run(() => source.downloadCancel(task.id))
                          }
                        >
                          {t("cancel")}
                        </Button>
                      ) : null}
                      {task.retryable ? (
                        <Button
                          type="button"
                          variant="outline"
                          size="default"
                          disabled={pending}
                          onClick={() =>
                            void run(() => source.downloadRetry(task.id))
                          }
                        >
                          {t("retry")}
                        </Button>
                      ) : null}
                      {task.phase === "completed" ? (
                        <>
                          <Button
                            type="button"
                            variant="outline"
                            size="default"
                            disabled={pending}
                            onClick={() =>
                              void run(() => source.downloadOpen(task.id))
                            }
                          >
                            {t(task.kind === "magnet" ? "openFolder" : "open")}
                          </Button>
                          <Button
                            type="button"
                            variant="ghost"
                            size="default"
                            disabled={pending}
                            onClick={() =>
                              void run(() => source.downloadReveal(task.id))
                            }
                          >
                            {t("reveal")}
                          </Button>
                        </>
                      ) : null}
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </CardContent>
      </Card>
    </Dialog>
  );
}
