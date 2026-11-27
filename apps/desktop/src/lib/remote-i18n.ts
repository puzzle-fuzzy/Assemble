import { i18n, registerTranslations, useTranslation } from "@assemble/display";
import en from "@/locales/remote.en.json";
import zhCN from "@/locales/remote.zh-CN.json";
import type { RemoteSession } from "./remote";

registerTranslations("remote", { "zh-CN": zhCN, en });
export const useRemoteTranslation = () => useTranslation("remote");
export const remoteText = (key: string) => i18n.t(key, { ns: "remote" });
export function remoteEnded(session: RemoteSession): string {
  return remoteText(session.endReason ? `end.${session.endReason}` : "ended");
}
export function remoteError(error: unknown): string {
  const code =
    typeof error === "object" && error !== null && "code" in error
      ? String(error.code)
      : "";
  return remoteText(
    [
      "REMOTE_CAPTURE_FAILED",
      "REMOTE_INPUT_FAILED",
      "REMOTE_BUSY",
      "DEVICE_OFFLINE",
      "REMOTE_ENDED",
      "REMOTE_UNSUPPORTED",
    ].includes(code)
      ? code
      : "failed",
  );
}
