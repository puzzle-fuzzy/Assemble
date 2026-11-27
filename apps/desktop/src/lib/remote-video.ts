// Annex B 没有 description；每次参考链中断都等待带 SPS/PPS 的 IDR，不能继续解码旧链。
import type { RemoteFrame } from "./remote";

export class RemoteVideoDecoder {
  private decoder: VideoDecoder;
  private stream = 0;
  private sequence = 0;
  private waiting = true;
  private requested = -Infinity;
  private pending: VideoFrame | null = null;
  private animation = 0;
  private closed = false;
  static async supported(): Promise<boolean> {
    if (typeof VideoDecoder === "undefined") return false;
    try {
      return (
        (
          await VideoDecoder.isConfigSupported({
            codec: "avc1.420028",
            optimizeForLatency: true,
          })
        ).supported === true
      );
    } catch {
      return false;
    }
  }
  constructor(
    private canvas: () => HTMLCanvasElement | null,
    private keyframe: () => void,
    private fallback: () => void,
  ) {
    this.decoder = new VideoDecoder({
      output: (frame) => {
        if (this.closed) {
          frame.close();
          return;
        }
        this.pending?.close();
        this.pending = frame;
        if (!this.animation)
          this.animation = requestAnimationFrame(() => {
            this.animation = 0;
            const image = this.pending;
            this.pending = null;
            if (!image) return;
            try {
              this.canvas()?.getContext("2d")?.drawImage(image, 0, 0);
            } finally {
              image.close();
            }
          });
      },
      error: () => this.fail(),
    });
  }
  receive(frame: RemoteFrame) {
    const video = frame.video;
    if (!video || this.closed || frame.sequence <= this.sequence) return;
    try {
      if (
        video.stream !== this.stream ||
        (this.sequence !== 0 && frame.sequence !== this.sequence + 1) ||
        this.decoder.decodeQueueSize > 3
      ) {
        this.decoder.reset();
        this.waiting = true;
        this.stream = video.stream;
      }
      this.sequence = frame.sequence;
      if (this.waiting && !video.keyframe) {
        if (performance.now() - this.requested >= 1000) {
          this.requested = performance.now();
          this.keyframe();
        }
        return;
      }
      if (this.waiting) {
        this.decoder.configure({
          codec: video.codec,
          optimizeForLatency: true,
        });
        this.waiting = false;
      }
      const data = Uint8Array.from(atob(video.data), (char) =>
        char.charCodeAt(0),
      );
      this.decoder.decode(
        new EncodedVideoChunk({
          type: video.keyframe ? "key" : "delta",
          timestamp: video.timestamp,
          data,
        }),
      );
    } catch {
      this.fail();
    }
  }
  private fail() {
    if (this.closed) return;
    this.close();
    this.fallback();
  }
  close() {
    this.closed = true;
    cancelAnimationFrame(this.animation);
    this.animation = 0;
    this.pending?.close();
    this.pending = null;
    if (this.decoder.state !== "closed") this.decoder.close();
  }
}
