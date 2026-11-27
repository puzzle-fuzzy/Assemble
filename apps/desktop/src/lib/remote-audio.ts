// 只保留当前小段播放计划；积压时丢弃旧声音，静音和退出立即停止所有已排期节点。
import type { AudioPlayback } from "./remote";
export class RemoteAudioPlayer {
  private next = 0;
  private sources = new Set<AudioBufferSourceNode>();
  private closed = false;
  private constructor(private context: AudioContext) {}
  static async open() {
    const context = new AudioContext({
      latencyHint: "interactive",
      sampleRate: 48000,
    });
    try {
      await context.resume();
      return new RemoteAudioPlayer(context);
    } catch (error) {
      await context.close();
      throw error;
    }
  }
  append(audio: AudioPlayback) {
    if (this.closed) return;
    const packets = audio.packets.slice(-6);
    if (
      this.next > this.context.currentTime + 0.16 ||
      this.next < this.context.currentTime
    ) {
      this.clear();
      this.next = this.context.currentTime + 0.04;
    }
    for (const packet of packets) {
      const buffer = this.context.createBuffer(2, 960, 48000);
      for (let channel = 0; channel < 2; channel++) {
        const samples = buffer.getChannelData(channel);
        for (let i = 0; i < samples.length; i++)
          samples[i] = Math.max(
            -1,
            Math.min(1, packet.samples[i * 2 + channel] ?? 0),
          );
      }
      const source = this.context.createBufferSource();
      source.buffer = buffer;
      source.connect(this.context.destination);
      source.onended = () => {
        this.sources.delete(source);
        source.disconnect();
      };
      this.sources.add(source);
      source.start(this.next);
      this.next += 0.02;
    }
  }
  private clear() {
    for (const source of this.sources) {
      try {
        source.stop();
      } catch {}
      source.disconnect();
    }
    this.sources.clear();
  }
  close() {
    this.closed = true;
    this.clear();
    void this.context.close();
  }
}
