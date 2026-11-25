import { Badge } from "@assemble/ui/badge";
import { Progress } from "@assemble/ui/progress";
import type { MediaClient } from "../client";
import { useMediaTranslation } from "../i18n";
import type { MediaJob } from "../model";

export function isActiveJob(job: MediaJob) {
  return job.status === "queued" || job.status === "running";
}
export async function watchMediaJob(
  client: MediaClient,
  initial: MediaJob,
  signal: AbortSignal,
  onJob: (job: MediaJob) => void,
): Promise<MediaJob> {
  let job = initial;
  onJob(job);
  while (isActiveJob(job)) {
    await new Promise<void>((resolve, reject) => {
      const stop = () => {
        clearTimeout(timer);
        reject(new DOMException("Aborted", "AbortError"));
      };
      const timer = setTimeout(() => {
        signal.removeEventListener("abort", stop);
        resolve();
      }, 1200);
      if (signal.aborted) stop();
      else signal.addEventListener("abort", stop, { once: true });
    });
    signal.throwIfAborted();
    job = await client.job(job.id, signal);
    signal.throwIfAborted();
    onJob(job);
  }
  return job;
}
export function MediaJobStatus({ job }: { job: MediaJob }) {
  const { t } = useMediaTranslation();
  const known =
    job.completedItems !== null &&
    job.totalItems !== null &&
    job.totalItems > 0;
  return (
    <div role="status" aria-live="polite" className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant={job.status === "failed" ? "destructive" : "secondary"}>
          {t(`import.${job.status}`)}
        </Badge>
        {job.stage && (
          <span className="text-sm">{t(`import.${job.stage}`)}</span>
        )}
      </div>
      {known && (
        <>
          <Progress
            value={Math.min(
              100,
              (100 * (job.completedItems ?? 0)) / (job.totalItems ?? 1),
            )}
            aria-label={t(`import.${job.stage ?? job.status}`)}
          />
          <p className="text-sm text-muted-foreground">
            {t("import.progress", {
              done: job.completedItems,
              total: job.totalItems,
            })}
          </p>
        </>
      )}
      <p className="break-all text-xs text-muted-foreground">
        {t("import.jobId", { id: job.id })}
      </p>
      {job.errorCode && (
        <p className="text-sm text-destructive">
          {t(`error.${job.errorCode}`, { defaultValue: t("error.generic") })}
        </p>
      )}
    </div>
  );
}
