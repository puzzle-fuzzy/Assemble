// 订阅 Rust 推来的状态，并在挂载时主动取一次当前状态。

import type { UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

import { type Bridge, bridge, run, toBridgeError } from "@/lib/bridge";
import { messageFor } from "@/lib/messages";
import type { Snapshot } from "@/lib/types";

export interface SnapshotState {
  /** 还没拿到任何状态时为 null。 */
  snapshot: Snapshot | null;
  /** 取初始状态失败时给用户看的一句话。 */
  loadError: string | null;
}

export function useSnapshot(source: Bridge = bridge): SnapshotState {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  // 只保存稳定错误码，重渲染时按当前语言生成文案，不保留诊断信息。
  const [failureCode, setFailureCode] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    let unlisten: UnlistenFn | undefined;

    run(() => source.snapshot()).then(
      (current) => {
        if (active) setSnapshot(current);
      },
      (error: unknown) => {
        if (active) setFailureCode(toBridgeError(error).code);
      },
    );

    run(() =>
      source.subscribe((next) => {
        if (active) setSnapshot(next);
      }),
    ).then(
      (stop) => {
        // 卸载之后订阅才成功：立刻取消，不留监听。
        if (active) unlisten = stop;
        else stop();
      },
      (error: unknown) => {
        // 订阅不上就收不到后续变化：如实告诉用户，而不是停留在旧状态。
        if (active) setFailureCode(toBridgeError(error).code);
      },
    );

    return () => {
      active = false;
      unlisten?.();
    };
  }, [source]);

  return {
    snapshot,
    loadError: failureCode
      ? messageFor(toBridgeError({ code: failureCode }))
      : null,
  };
}
