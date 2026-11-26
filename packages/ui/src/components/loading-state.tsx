import { Spinner } from "@assemble/ui/components/spinner";
import { useUiTranslation } from "@assemble/ui/lib/i18n";
import { cn } from "cn";
import { useEffect, useState } from "react";

/** 短请求不显示转圈，避免切页时加载指示本身也闪一下。 */
export function LoadingState({
  label,
  className,
}: {
  label?: string;
  className?: string;
}) {
  const { t } = useUiTranslation();
  const description = label ?? t("loading");
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    const timer = setTimeout(() => setVisible(true), 200);
    return () => clearTimeout(timer);
  }, []);
  return (
    <div
      data-slot="loading-state"
      role="status"
      aria-busy="true"
      aria-label={description}
      className={cn(
        "flex w-full items-center justify-center bg-background text-muted-foreground",
        className,
      )}
    >
      {visible ? (
        <>
          {/* 页面可能全局压缩动画时长，这里仍保留可辨认的慢速忙碌指示。 */}
          <Spinner
            aria-hidden="true"
            role="presentation"
            className="motion-reduce:animate-[spin_6s_linear_infinite]!"
          />
          <span className="sr-only">{description}</span>
        </>
      ) : null}
    </div>
  );
}
