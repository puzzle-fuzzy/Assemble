import { Button } from "@assemble/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardHeader,
  CardTitle,
} from "@assemble/ui/card";
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
import { Field, FieldGroup, FieldLabel as Label } from "@assemble/ui/field";
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
  const [url, setUrl] = useState("");
  const [page, setPage] = useState("");
  const [preview, setPreview] = useState<DownloadPreview | null>(null);
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
      setPreview(value);
      setVariant(value.variants[0]?.id ?? "");
      setUrl("");
      setPage("");
    }
  }
  return (
    <Dialog open={compose} onOpenChange={setCompose}>
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
                  url,
                  pageUrl: page || null,
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
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
            <Field>
              <Label htmlFor={`${id}-url`}>
                {t(kind === "hls" ? "hlsAddress" : "address")}
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
            <div className="flex flex-wrap gap-2">
              <Button type="submit" disabled={pending}>
                {t(pending ? "parsing" : "parse")}
              </Button>
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
          <p
            id={`${id}-error`}
            role="alert"
            className="text-sm text-destructive"
          >
            {downloadError(error)}
          </p>
        ) : null}
        {preview ? (
          <div className="flex flex-col gap-3 rounded-lg border p-3">
            <p className="break-words text-sm font-medium">{preview.title}</p>
            <p className="text-xs text-muted-foreground">
              {preview.sourceHost}
            </p>
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
            <Button
              disabled={pending || !view?.toolsAvailable}
              onClick={() =>
                void run(async () => {
                  await source.downloadStart(preview.id, variant);
                  setPreview(null);
                  setCompose(false);
                })
              }
            >
              {t("start")}
            </Button>
          </div>
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
              <Button type="button" variant="outline" size="sm">
                {t("new")}
              </Button>
            </DialogTrigger>
            <Button
              type="button"
              variant="outline"
              size="sm"
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
            <p role="alert" className="text-sm text-destructive">
              {downloadError(error)}
            </p>
          ) : null}
          {!view?.tasks.length ? (
            <div className="py-8 text-center">
              <p className="text-sm font-medium">{t("empty")}</p>
            </div>
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
                    className="flex flex-col gap-2 py-4 first:pt-0 last:pb-0"
                  >
                    <p className="break-words text-sm font-medium">
                      {task.fileName ?? task.title}
                    </p>
                    <p
                      className="text-xs text-muted-foreground"
                      role={active ? undefined : "status"}
                    >
                      {t(task.phase)}
                    </p>
                    {active ? (
                      <Progress
                        value={progress}
                        aria-label={`${task.title} · ${t(task.phase)}`}
                      />
                    ) : null}
                    <div className="flex flex-wrap gap-2 text-xs text-muted-foreground">
                      <span>{formatSize(task.receivedBytes)}</span>
                      {task.totalSegments !== null ? (
                        <span>
                          {t("parts", {
                            done: task.completedSegments,
                            total: task.totalSegments,
                          })}
                        </span>
                      ) : null}
                      {task.bytesPerSecond > 0 ? (
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
                      {active ? (
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
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
                          size="sm"
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
                            size="sm"
                            disabled={pending}
                            onClick={() =>
                              void run(() => source.downloadOpen(task.id))
                            }
                          >
                            {t("open")}
                          </Button>
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
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
