// 传输的说法：一条传输现在怎么样了、失败是因为什么、文件多大。
//
// 主窗口的"传输"卡片与发送面板用的是同一套说法，所以放在这里，不各写一份。
// 同一件事按方向与另一头各说各的：收到的、发出去的、存进云盘的、从云盘取回的。

import type { TransferFailure, TransferView } from "@/lib/types";

/** 文件大小怎么显示。单文件不超过 100 MB，所以最大到 MB 就够了。 */
export function formatSize(bytes: number): string {
  const kilobytes = bytes / 1024;
  if (kilobytes < 1) return `${bytes} B`;
  if (kilobytes < 1024)
    return `${kilobytes < 10 ? kilobytes.toFixed(1) : Math.round(kilobytes)} KB`;
  return `${(kilobytes / 1024).toFixed(1)} MB`;
}

/** 空间怎么显示：云盘的容量以 GB 计。 */
export function formatSpace(bytes: number): string {
  const gigabytes = bytes / (1024 * 1024 * 1024);
  return gigabytes >= 1 ? `${gigabytes.toFixed(1)} GB` : formatSize(bytes);
}

/** 失败原因的说法。同一件事，收与发两边的说法不一样：谁的磁盘、谁没收到。 */
const FAILURE_NAMES: Record<
  TransferFailure,
  { received: string | null; sent: string | null }
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
  quotaExceeded: { received: null, sent: "云盘空间不够" },
  vanished: {
    received: "云盘里已经没有这个文件了",
    sent: null,
  },
};

/** 一条传输涉及的是哪一类事。 */
type Kind = "received" | "sent" | "stored" | "fetched";

function kindOf(transfer: TransferView): Kind {
  if (transfer.target === "drive") {
    return transfer.direction === "out" ? "stored" : "fetched";
  }
  return transfer.direction === "in" ? "received" : "sent";
}

/** 每一类事在各个阶段怎么说。 */
const WORDS: Record<
  Kind,
  { queued: string; running: string; done: string; failed: string }
> = {
  received: {
    queued: "等待接收…",
    running: "正在接收",
    done: "已保存",
    failed: "接收失败",
  },
  sent: {
    queued: "等待发送…",
    running: "正在发送",
    done: "对方已收下",
    failed: "发送失败",
  },
  stored: {
    queued: "等待上传…",
    running: "正在上传到云盘",
    done: "已存到云盘",
    failed: "没能存进云盘",
  },
  fetched: {
    queued: "等待取回…",
    running: "正在从云盘取回",
    done: "已取回",
    failed: "取回失败",
  },
};

/** 一条传输现在怎么样了，用一句话说清楚。 */
export function transferStatus(transfer: TransferView): string {
  const kind = kindOf(transfer);
  const words = WORDS[kind];
  const percent = Math.floor(transfer.progress * 100);
  switch (transfer.phase) {
    case "queued":
      return words.queued;
    case "inProgress":
      return `${words.running} ${percent}%`;
    case "waiting":
      // 只有发给设备的文件才有"等对方接收"这一步。
      return "已发出，等对方接收";
    case "complete":
      return words.done;
    case "failed": {
      const names = transfer.failure ? FAILURE_NAMES[transfer.failure] : null;
      const reason =
        (kind === "received" || kind === "fetched"
          ? names?.received
          : names?.sent) ?? "原因不明";
      return `${words.failed}：${reason}。`;
    }
    case "declined":
      return "对方拒收了。";
    case "cancelled":
      return kind === "received" ? "对方撤回了，没有收到。" : "已取消。";
    case "expired":
      return kind === "received"
        ? "过了时限，没有收到。"
        : "对方没有在时限内接收。";
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
  if (transfers.some((t) => t.phase === "queued" || t.phase === "inProgress"))
    return "sending";
  return transfers.some((t) => t.phase === "failed") ? "failed" : "delivered";
}
