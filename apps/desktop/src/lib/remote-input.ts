// 参考 DeskLink 的归一化坐标和有界输入：一个在途请求，移动事件只保留最新位置。
import type { RemoteInput, SequencedInput } from "./remote";
export function remotePoint(
  x: number,
  y: number,
  bounds: Pick<DOMRect, "left" | "top" | "width" | "height">,
): RemoteInput | null {
  if (
    bounds.width <= 0 ||
    bounds.height <= 0 ||
    x < bounds.left ||
    y < bounds.top ||
    x > bounds.left + bounds.width ||
    y > bounds.top + bounds.height
  )
    return null;
  return {
    type: "move",
    x: Math.round(((x - bounds.left) / bounds.width) * 65535),
    y: Math.round(((y - bounds.top) / bounds.height) * 65535),
  };
}
export class RemoteInputQueue {
  private queue: Omit<SequencedInput, "sequence">[] = [];
  private sequence = 0;
  private sending = false;
  private stopped = false;
  private waiters = new Set<(sent: boolean) => void>();
  constructor(
    private send: (events: SequencedInput[]) => Promise<void>,
    private failed: () => void,
  ) {}
  push(input: RemoteInput, displayId?: number) {
    if (this.stopped) return;
    const pending = displayId === undefined ? { input } : { input, displayId };
    if (
      input.type === "move" &&
      this.queue.at(-1)?.input.type === "move" &&
      this.queue.at(-1)?.displayId === displayId
    )
      this.queue[this.queue.length - 1] = pending;
    else this.queue.push(pending);
    if (this.queue.length > 128) {
      this.stop();
      this.failed();
      return;
    }
    void this.flush();
  }
  stop() {
    this.stopped = true;
    this.queue = [];
    this.resolve(false);
  }
  release() {
    this.queue = this.queue.filter(
      ({ input }) =>
        !["move", "button", "key", "scroll", "text"].includes(input.type),
    );
    this.push({ type: "releaseAll" });
  }
  drain(): Promise<boolean> {
    if (this.stopped) return Promise.resolve(false);
    if (!this.sending && !this.queue.length) return Promise.resolve(true);
    return new Promise((resolve) => this.waiters.add(resolve));
  }
  private resolve(sent: boolean) {
    for (const resolve of this.waiters) resolve(sent);
    this.waiters.clear();
  }
  private async flush() {
    if (this.sending || this.stopped) return;
    this.sending = true;
    try {
      while (this.queue.length && !this.stopped) {
        const batch = this.queue
          .splice(0, 32)
          .map((pending) => ({ sequence: ++this.sequence, ...pending }));
        await this.send(batch);
      }
    } catch {
      if (!this.stopped) {
        this.stop();
        this.failed();
      }
    } finally {
      this.sending = false;
      if (!this.queue.length) this.resolve(!this.stopped);
    }
  }
}
export function clipboardShortcut(input: {
  key: string;
  code: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
}): boolean {
  return (
    !input.altKey &&
    input.ctrlKey !== input.metaKey &&
    (input.code === "KeyV" || input.key.toLowerCase() === "v")
  );
}
const named = new Set([
  "Enter",
  "Tab",
  "Backspace",
  "Delete",
  "Insert",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "Control",
  "Shift",
  "Alt",
  "Meta",
  ...Array.from({ length: 12 }, (_, i) => `F${i + 1}`),
]);
export function remoteKey(key: string, code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3).toLowerCase();
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  return named.has(key) ||
    ([...key].length === 1 && (key.codePointAt(0) ?? 0) > 31)
    ? key
    : null;
}
