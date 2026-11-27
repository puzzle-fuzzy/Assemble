import { FieldError } from "@assemble/ui/field";
import { Switch } from "@assemble/ui/switch";
import { useState } from "react";
import { type Bridge, toBridgeError } from "@/lib/bridge";
import {
  downloadError,
  downloadText as t,
  useDownloadTranslation,
} from "@/lib/download-i18n";

export function AssistantDownloadsSetting({
  source,
  enabled,
}: {
  source: Bridge;
  enabled: boolean;
}) {
  useDownloadTranslation();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function change(value: boolean) {
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      await source.downloadSetAssistantAllowed(value);
    } catch (cause) {
      setError(toBridgeError(cause).code);
    } finally {
      setPending(false);
    }
  }
  return (
    <div className="flex flex-col gap-2 pt-3">
      <div className="flex items-center justify-between gap-3">
        <p className="text-sm font-medium">{t("assistantAllowed")}</p>
        <Switch
          aria-label={t("assistantAllowed")}
          checked={enabled}
          disabled={pending}
          onCheckedChange={(checked) => void change(checked)}
        />
      </div>
      {error ? <FieldError>{downloadError(error)}</FieldError> : null}
    </div>
  );
}
