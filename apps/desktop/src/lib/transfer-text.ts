// 传输的说法：一条传输现在怎么样了、失败是因为什么、文件多大。
//
// 主窗口的"传输"卡片与发送面板用的是同一套说法，所以放在这里，不各写一份。
// 同一件事按方向与另一头各说各的：收到的、发出去的、存进云盘的、从云盘取回的。

import { text, textLocale } from "@/lib/text-i18n";
import type { TransferView } from "@/lib/types";
import zhCN from "@/locales/text.zh-CN.json";

function size(value: number, unit: string, decimals = 0): string {
  return text("transfer.size", {
    value: new Intl.NumberFormat(textLocale(), {
      useGrouping: false,
      minimumFractionDigits: decimals,
      maximumFractionDigits: decimals,
    }).format(value),
    unit,
  });
}

/** 文件大小怎么显示。单文件不超过 100 MB，所以最大到 MB 就够了。 */
export function formatSize(bytes: number): string {
  const kilobytes = bytes / 1024;
  if (kilobytes < 1) return size(bytes, "B");
  if (kilobytes < 1024) return size(kilobytes, "KB", kilobytes < 10 ? 1 : 0);
  return size(kilobytes / 1024, "MB", 1);
}

/** 空间怎么显示：云盘的容量以 GB 计。 */
export function formatSpace(bytes: number): string {
  const gigabytes = bytes / (1024 * 1024 * 1024);
  return gigabytes >= 1 ? size(gigabytes, "GB", 1) : formatSize(bytes);
}

/** 一条传输涉及的是哪一类事。 */
type Kind = "received" | "sent" | "stored" | "fetched";

function kindOf(transfer: TransferView): Kind {
  if (transfer.target === "drive") {
    return transfer.direction === "out" ? "stored" : "fetched";
  }
  return transfer.direction === "in" ? "received" : "sent";
}

/** 一条传输现在怎么样了，用一句话说清楚。 */
export function transferStatus(transfer: TransferView): string {
  const kind = kindOf(transfer);
  const state = (phase: "queued" | "running" | "done" | "failed") =>
    text(`transfer.states.${kind}.${phase}`);
  const percent = Math.floor(transfer.progress * 100);
  switch (transfer.phase) {
    case "queued":
      return state("queued");
    case "inProgress":
      return text("transfer.progress", { state: state("running"), percent });
    case "paused":
      if (transfer.failure === "fileUnreadable")
        return text("transfer.sourceMissing");
      return text("transfer.paused", { percent });
    case "waiting":
      // 只有发给设备的文件才有"等对方接收"这一步。
      return text("transfer.waiting");
    case "complete":
      return state("done");
    case "failed": {
      if (transfer.failure === "interrupted")
        return text("transfer.interrupted");
      const names = transfer.failure
        ? zhCN.transfer.failures[transfer.failure]
        : null;
      const side =
        kind === "received" || kind === "fetched" ? "received" : "sent";
      const reason =
        names && Object.hasOwn(names, side)
          ? text(`transfer.failures.${transfer.failure}.${side}`)
          : text("transfer.unknownReason");
      return text("transfer.failed", { state: state("failed"), reason });
    }
    case "declined":
      return text("transfer.declined");
    case "cancelled":
      return text(
        kind === "received" ? "transfer.withdrawn" : "transfer.cancelled",
      );
    case "expired":
      return text(
        kind === "received"
          ? "transfer.expiredReceived"
          : "transfer.expiredSent",
      );
  }
}

/** 这条传输收下来的文件在这台电脑上吗：在的话才能"在文件夹中显示"。
 *  存进云盘的文件原件本来就在这台电脑上，但它的位置不经过界面，所以不在此列。 */
export function isSavedHere(transfer: TransferView): boolean {
  return transfer.phase === "complete" && transfer.direction === "in";
}

/** 一批发出去的文件整体怎么样了。还没全部传完时为 `sending`。 */
export type Verdict = "sending" | "delivered" | "failed";

export function verdict(transfers: TransferView[]): Verdict | null {
  if (transfers.length === 0) return null;
  if (
    transfers.some(
      (t) =>
        t.phase === "queued" ||
        t.phase === "inProgress" ||
        t.phase === "paused",
    )
  )
    return "sending";
  return transfers.some((t) => t.phase === "failed") ? "failed" : "delivered";
}
