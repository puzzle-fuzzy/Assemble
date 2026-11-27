import { useDisplayPreferences } from "@assemble/display";
import { Alert, AlertDescription, AlertTitle } from "@assemble/ui/alert";
import { Badge } from "@assemble/ui/badge";
import { Button } from "@assemble/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@assemble/ui/dialog";
import { DisplayPreferencesMenu } from "@assemble/ui/display-preferences";
import { Skeleton } from "@assemble/ui/skeleton";
import {
  DownloadIcon,
  FileTextIcon,
  LaptopIcon,
  MonitorIcon,
  SmartphoneIcon,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import {
  type Artifact,
  downloadPath,
  type Manifest,
  platforms,
} from "./lib/catalog";
import { useReleasesTranslation } from "./lib/i18n";
import { loadReleases } from "./lib/load";

const icons = {
  windows: MonitorIcon,
  macos: LaptopIcon,
  ios: SmartphoneIcon,
  android: SmartphoneIcon,
};
type Result =
  | { phase: "loading" }
  | { phase: "error" }
  | { phase: "ready"; manifest: Manifest };

export function App() {
  const { t } = useReleasesTranslation();
  const { language } = useDisplayPreferences();
  const [attempt, setAttempt] = useState(0);
  const [result, setResult] = useState<Result>({ phase: "loading" });
  const [selected, setSelected] = useState<Artifact | null>(null);
  const selectedTrigger = useRef<HTMLButtonElement | null>(null);
  useEffect(() => {
    document.title = t("title");
  }, [t]);
  useEffect(() => {
    // 重试序号是这次读取的身份，切换语言不应重新请求发布资料。
    void attempt;
    const controller = new AbortController();
    setResult({ phase: "loading" });
    setSelected(null);
    void loadReleases(controller.signal).then(
      (manifest) => {
        if (!controller.signal.aborted) setResult({ phase: "ready", manifest });
      },
      () => {
        if (!controller.signal.aborted) setResult({ phase: "error" });
      },
    );
    return () => controller.abort();
  }, [attempt]);
  const size = (bytes: number) => {
    const unit =
      bytes >= 1024 ** 3
        ? "gigabyte"
        : bytes >= 1024 ** 2
          ? "megabyte"
          : bytes >= 1024
            ? "kilobyte"
            : "byte";
    const divisor = {
      byte: 1,
      kilobyte: 1024,
      megabyte: 1024 ** 2,
      gigabyte: 1024 ** 3,
    }[unit];
    return new Intl.NumberFormat(language, {
      style: "unit",
      unit,
      unitDisplay: "short",
      maximumFractionDigits: 1,
    }).format(bytes / divisor);
  };
  return (
    <div className="releases-shell flex flex-col">
      <header className="flex min-h-16 flex-wrap items-center justify-between gap-3 py-3">
        <a href="/download/" className="text-base font-semibold">
          Assemble
        </a>
        <nav aria-label={t("navigation")} className="flex items-center gap-2">
          <Button asChild variant="ghost" size="sm">
            <a href="/movies/">{t("movies")}</a>
          </Button>
          <DisplayPreferencesMenu />
        </nav>
      </header>
      <main
        className="flex min-w-0 flex-1 flex-col gap-8 py-10 sm:py-16"
        aria-busy={result.phase === "loading"}
      >
        <h1 className="releases-heading">{t("title")}</h1>
        {result.phase === "error" ? (
          <Alert variant="destructive">
            <AlertTitle>{t("errorTitle")}</AlertTitle>
            <AlertDescription>
              <p>{t("errorMessage")}</p>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setAttempt((value) => value + 1)}
              >
                {t("retry")}
              </Button>
            </AlertDescription>
          </Alert>
        ) : null}
        {result.phase === "loading" ? (
          <div
            role="status"
            aria-label={t("loading")}
            className="flex flex-col gap-6"
          >
            {platforms.map((platform) => (
              <Skeleton key={platform} className="h-24 w-full" />
            ))}
          </div>
        ) : result.phase === "ready" ? (
          <div className="flex flex-col divide-y rounded-lg border">
            {platforms.map((platform) => {
              const Icon = icons[platform];
              const artifacts = result.manifest.artifacts.filter(
                (item) => item.platform === platform,
              );
              return (
                <section
                  key={platform}
                  aria-labelledby={`platform-${platform}`}
                  className="flex min-w-0 flex-col gap-5 p-5 sm:flex-row sm:gap-8 sm:p-6"
                >
                  <h2
                    id={`platform-${platform}`}
                    className="flex shrink-0 items-center gap-3 text-base font-medium sm:w-36"
                  >
                    <Icon className="size-5" aria-hidden="true" />
                    {t(`platform.${platform}`)}
                  </h2>
                  {artifacts.length === 0 ? (
                    <div className="flex min-h-8 flex-1 items-center sm:justify-end">
                      <Badge variant="secondary">{t("unreleased")}</Badge>
                    </div>
                  ) : (
                    <ul className="flex min-w-0 flex-1 flex-col gap-5">
                      {artifacts.map((artifact) => {
                        const context = {
                          platform: t(`platform.${platform}`),
                          architecture: t(
                            `architecture.${artifact.architecture}`,
                          ),
                          version: artifact.version,
                        };
                        return (
                          <li
                            key={artifact.sha256}
                            className="flex min-w-0 flex-wrap items-center justify-between gap-4"
                          >
                            <div className="flex min-w-0 flex-col gap-1">
                              <p className="text-sm font-medium">
                                {context.architecture}
                              </p>
                              <p className="break-words text-sm text-muted-foreground">
                                {artifact.version} · {size(artifact.size)}
                              </p>
                            </div>
                            <div className="ms-auto flex shrink-0 flex-wrap items-center gap-2">
                              <Button
                                variant="ghost"
                                size="sm"
                                aria-label={t("verifyFor", context)}
                                onClick={(event) => {
                                  selectedTrigger.current = event.currentTarget;
                                  setSelected(artifact);
                                }}
                              >
                                <FileTextIcon
                                  data-icon="inline-start"
                                  aria-hidden="true"
                                />
                                {t("verify")}
                              </Button>
                              <Button asChild size="sm">
                                <a
                                  href={downloadPath(artifact)}
                                  download={artifact.fileName}
                                  aria-label={t("downloadFor", context)}
                                >
                                  <DownloadIcon
                                    data-icon="inline-start"
                                    aria-hidden="true"
                                  />
                                  {t("download")}
                                </a>
                              </Button>
                            </div>
                          </li>
                        );
                      })}
                    </ul>
                  )}
                </section>
              );
            })}
          </div>
        ) : null}
      </main>
      <footer className="flex justify-end py-6 text-sm text-muted-foreground">
        <a
          href="https://github.com/puzzle-fuzzy/Assemble"
          target="_blank"
          rel="noreferrer"
        >
          GitHub
        </a>
      </footer>
      <Dialog
        open={selected !== null}
        onOpenChange={(open) => {
          if (!open) setSelected(null);
        }}
      >
        <DialogContent
          className="max-h-[90dvh] overflow-y-auto sm:max-w-xl"
          aria-describedby={undefined}
          onCloseAutoFocus={(event) => {
            if (selectedTrigger.current?.isConnected) {
              event.preventDefault();
              selectedTrigger.current.focus();
            }
          }}
        >
          <DialogHeader>
            <DialogTitle>{t("verify")}</DialogTitle>
          </DialogHeader>
          {selected ? (
            <dl className="flex min-w-0 flex-col gap-4 text-sm">
              <div>
                <dt className="text-muted-foreground">{t("file")}</dt>
                <dd className="break-all">{selected.fileName}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">{t("size")}</dt>
                <dd>{size(selected.size)}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">SHA-256</dt>
                <dd>
                  <code className="break-all select-all">
                    {selected.sha256}
                  </code>
                </dd>
              </div>
              <div>
                <dt className="text-muted-foreground">{t("publishedAt")}</dt>
                <dd>
                  <time dateTime={selected.publishedAt}>
                    {new Intl.DateTimeFormat(language, {
                      dateStyle: "medium",
                      timeStyle: "short",
                      timeZone: "UTC",
                    }).format(new Date(selected.publishedAt))}{" "}
                    UTC
                  </time>
                </dd>
              </div>
            </dl>
          ) : null}
        </DialogContent>
      </Dialog>
    </div>
  );
}
