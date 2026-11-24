// 传输的说法：一条传输现在怎么样了、失败是因为什么、文件多大。
//
// 主窗口的"传输"卡片与发送面板用的是同一套说法，所以放在这里，不各写一份。

import type { TransferFailure, TransferView } from "@/lib/types";

/** 文件大小怎么显示。单文件不超过 100 MB，所以最大到 MB 就够了。 */
export function formatSize(bytes: number): string {
  const kilobytes = bytes / 1024;
  if (kilobytes < 1) return `${bytes} B`;
  if (kilobytes < 1024)
    return `${kilobytes < 10 ? kilobytes.toFixed(1) : Math.round(kilobytes)} KB`;
  return `${(kilobytes / 1024).toFixed(1)} MB`;
}

/** 失败原因的说法。同一件事，收与发两边的说法不一样：谁的磁盘、谁没收到。 */
const FAILURE_NAMES: Record<
  TransferFailure,
  { received: string | null; sent: string }
> = {
  diskFull: {
    received: "这台电脑的磁盘已满",
    sent: "对方设备的磁盘已满",
  },
  writeFailed: {
    received: "文件没能写进接收文件夹",
    sent: "对方设备没能把文件写下来",
  },
  verificationFailed: {
    received: "收到的内容与发出的对不上",
    sent: "对方收到的内容与发出的对不上",
  },
  cancelled: { received: "已取消", sent: "已取消" },
  deviceOffline: {
    received: null,
    sent: "那台设备刚刚下线了",
  },
  tooLarge: { received: null, sent: "超过 100 MB" },
  fileUnreadable: {
    received: null,
    sent: "文件读不出来，可能已被移走或改动",
  },
  rejected: {
    received: null,
    sent: "服务端拒绝了这次发送，请重新选择文件",
  },
};

/** 一条传输现在怎么样了，用一句话说清楚。 */
export function transferStatus(transfer: TransferView): string {
  const receiving = transfer.direction === "in";
  const percent = Math.floor(transfer.progress * 100);
  switch (transfer.phase) {
    case "queued":
      return receiving ? "等待接收…" : "等待发送…";
    case "inProgress":
      return receiving ? `正在接收 ${percent}%` : `正在发送 ${percent}%`;
    case "waiting":
      return "已发出，等对方接收";
    case "complete":
      return receiving ? "已保存" : "对方已收下";
    case "failed": {
      const words = transfer.failure ? FAILURE_NAMES[transfer.failure] : null;
      const reason = (receiving ? words?.received : words?.sent) ?? "原因不明";
      return `${receiving ? "接收失败" : "发送失败"}：${reason}。`;
    }
    case "declined":
      return "对方拒收了。";
    case "cancelled":
      return receiving ? "对方撤回了，没有收到。" : "已撤回。";
    case "expired":
      return receiving ? "过了时限，没有收到。" : "对方没有在时限内接收。";
  }
}

/** 一批发出去的文件整体怎么样了。还没全部传完时为 `sending`。 */
export type Verdict = "sending" | "delivered" | "failed";

export function verdict(transfers: TransferView[]): Verdict | null {
  if (transfers.length === 0) return null;
  if (transfers.some((t) => t.phase === "queued" || t.phase === "inProgress"))
    return "sending";
  return transfers.some((t) => t.phase === "failed") ? "failed" : "delivered";
}
