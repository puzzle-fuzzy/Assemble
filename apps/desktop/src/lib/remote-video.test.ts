import { afterEach, expect, test } from "bun:test";
import type { RemoteFrame } from "./remote";
import { RemoteVideoDecoder } from "./remote-video";

const original = Object.getOwnPropertyDescriptor(globalThis, "VideoDecoder");
const originalChunk = Object.getOwnPropertyDescriptor(
  globalThis,
  "EncodedVideoChunk",
);
afterEach(() => {
  if (original) Object.defineProperty(globalThis, "VideoDecoder", original);
  else Reflect.deleteProperty(globalThis, "VideoDecoder");
  if (originalChunk)
    Object.defineProperty(globalThis, "EncodedVideoChunk", originalChunk);
  else Reflect.deleteProperty(globalThis, "EncodedVideoChunk");
});
function frame(sequence: number, keyframe: boolean): RemoteFrame {
  return {
    sequence,
    displayId: 1,
    width: 16,
    height: 16,
    jpeg: "",
    displays: [{ id: 1, name: "Display", width: 16, height: 16 }],
    video: {
      stream: 1,
      codec: "avc1.420028",
      keyframe,
      timestamp: sequence * 33333,
      data: "AAAAAWc=",
    },
  };
}
test("参考链断层后丢弃增量帧，有界请求新关键帧；积压与退出释放解码器", () => {
  const accepted: string[] = [];
  let native: FakeDecoder | null = null;
  class FakeDecoder {
    state = "unconfigured";
    decodeQueueSize = 0;
    constructor() {
      native = this;
    }
    configure() {
      this.state = "configured";
    }
    decode(chunk: { type: string }) {
      accepted.push(chunk.type);
    }
    reset() {
      this.state = "unconfigured";
      this.decodeQueueSize = 0;
    }
    close() {
      this.state = "closed";
    }
  }
  Object.defineProperty(globalThis, "VideoDecoder", {
    value: FakeDecoder,
    configurable: true,
  });
  Object.defineProperty(globalThis, "EncodedVideoChunk", {
    value: class {
      type: string;
      constructor(init: { type: string }) {
        this.type = init.type;
      }
    },
    configurable: true,
  });
  let requested = 0;
  const decoder = new RemoteVideoDecoder(
    () => null,
    () => requested++,
    () => {
      throw new Error("unexpected fallback");
    },
  );
  decoder.receive(frame(1, true));
  decoder.receive(frame(3, false));
  decoder.receive(frame(4, false));
  expect(accepted).toEqual(["key"]);
  expect(requested).toBe(1);
  decoder.receive(frame(5, true));
  expect(accepted).toEqual(["key", "key"]);
  if (!native) throw new Error("missing decoder");
  (native as FakeDecoder).decodeQueueSize = 4;
  decoder.receive(frame(6, false));
  expect(accepted).toEqual(["key", "key"]);
  decoder.close();
  expect((native as FakeDecoder).state).toBe("closed");
});
