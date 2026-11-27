// 自动更新的状态怎么说给人听。状态本身由 Rust 那边决定，这里只挑一句话。

import { text } from "@/lib/text-i18n";
import { formatSize } from "@/lib/transfer-text";
import type { UpdateView } from "@/lib/types";

export function updateStatus(view: UpdateView): string {
  switch (view.state) {
    case "unsupported":
      return text("update.unsupported");
    case "idle":
      return text("update.idle");
    case "checking":
      return text("update.checking");
    case "upToDate":
      return text("update.upToDate");
    case "downloading": {
      const received = view.received ?? 0;
      return typeof view.total === "number"
        ? text("update.downloading", {
            version: view.version ?? "",
            received: formatSize(received),
            total: formatSize(view.total),
          })
        : text("update.downloadingUnknownTotal", {
            version: view.version ?? "",
            received: formatSize(received),
          });
    }
    case "ready":
      return text("update.ready", { version: view.version ?? "" });
    case "failed":
      return text("update.failed", { code: view.error ?? "UNKNOWN" });
  }
}
