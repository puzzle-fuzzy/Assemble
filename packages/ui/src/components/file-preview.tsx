import {
  type ReactNode,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import type { PreviewResource } from "../lib/file-preview";
import { useUiTranslation } from "../lib/i18n";
import { Button } from "./button";

/** 弹窗关闭时卸载此内容：停止媒体、取消读取、释放仅属于本次预览的资源。 */
export function FilePreviewContent({
  name,
  load,
  download,
}: {
  name: string;
  load: (signal: AbortSignal) => Promise<PreviewResource>;
  download: ReactNode;
}) {
  const { t } = useUiTranslation();
  const [resource, setResource] = useState<PreviewResource | null>(null);
  const [failed, setFailed] = useState(false);
  const [revision, setRevision] = useState(0);
  const media = useRef<HTMLMediaElement | null>(null);
  const attachMedia = useCallback((node: HTMLMediaElement | null) => {
    if (media.current && media.current !== node) {
      media.current.pause();
      media.current.removeAttribute("src");
      media.current.load();
    }
    media.current = node;
  }, []);
  // biome-ignore lint/correctness/useExhaustiveDependencies: 重试代次有意重新启动读取并清理前一次资源。
  useEffect(() => {
    const abort = new AbortController();
    let current: PreviewResource | null = null;
    setResource(null);
    setFailed(false);
    void load(abort.signal)
      .then((value) => {
        if (abort.signal.aborted) {
          value.dispose?.();
          return;
        }
        current = value;
        setResource(value);
      })
      .catch(() => {
        if (!abort.signal.aborted) setFailed(true);
      });
    return () => {
      abort.abort();
      media.current?.pause();
      media.current?.removeAttribute("src");
      media.current?.load();
      current?.dispose?.();
    };
  }, [load, revision]);
  const preview = resource?.preview;
  return (
    <div className="flex min-h-32 min-w-0 flex-col gap-4">
      {failed ? (
        <div role="alert" className="flex flex-col items-start gap-3">
          <p>{t("filePreview.failed")}</p>
          <Button
            variant="outline"
            onClick={() => setRevision((value) => value + 1)}
          >
            {t("filePreview.retry")}
          </Button>
        </div>
      ) : !preview ? (
        <p role="status">{t("loading")}</p>
      ) : preview.kind === "text" ? (
        <>
          <section
            // biome-ignore lint/a11y/noNoninteractiveTabindex: 长文本滚动区域需要键盘入口。
            tabIndex={0}
            aria-label={name}
            className="max-h-[65dvh] overflow-auto rounded-md bg-muted p-3 text-sm"
          >
            <pre className="whitespace-pre-wrap break-words">
              {preview.text}
            </pre>
          </section>
          {preview.truncated && (
            <p className="text-sm text-muted-foreground">
              {t("filePreview.truncated")}
            </p>
          )}
        </>
      ) : preview.kind === "image" && resource.src ? (
        <img
          src={resource.src}
          alt={name}
          className="max-h-[65dvh] max-w-full object-contain"
          referrerPolicy="no-referrer"
          onError={() => setFailed(true)}
        />
      ) : preview.kind === "video" && resource.src ? (
        // biome-ignore lint/a11y/useMediaCaption: 用户文件没有字幕轨道，不虚构字幕。
        <video
          ref={attachMedia}
          src={resource.src}
          controls
          playsInline
          preload="metadata"
          aria-label={name}
          className="max-h-[65dvh] w-full"
          onError={() => setFailed(true)}
        />
      ) : preview.kind === "audio" && resource.src ? (
        // biome-ignore lint/a11y/useMediaCaption: 用户音频没有文字稿，不虚构字幕。
        <audio
          ref={attachMedia}
          src={resource.src}
          controls
          preload="metadata"
          aria-label={name}
          className="w-full"
          onError={() => setFailed(true)}
        />
      ) : (
        <p>{t("filePreview.unsupported")}</p>
      )}
      <div className="flex justify-end gap-2">{download}</div>
    </div>
  );
}
