import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { createPosterMotion } from "./poster-motion";

type PendingAnimation = {
  element: Element;
  keyframes: Keyframe[];
  options: KeyframeAnimationOptions;
  finish: () => void;
  cancelled: boolean;
};
const pending: PendingAnimation[] = [];
const originalAnimate = Object.getOwnPropertyDescriptor(
  Element.prototype,
  "animate",
);
const originalMatchMedia = window.matchMedia;
let reduced = false;
let host: HTMLElement;

function image(rect: DOMRect) {
  const result = document.createElement("img");
  result.getBoundingClientRect = () => rect;
  Object.defineProperties(result, {
    complete: { configurable: true, value: true },
    naturalWidth: { configurable: true, value: 500 },
  });
  return result;
}

function fixture() {
  const source = image(new DOMRect(12, 24, 100, 150));
  const overlay = document.createElement("div");
  overlay.dataset.slot = "dialog-overlay";
  const content = document.createElement("div");
  const preview = image(new DOMRect(300, 100, 400, 600));
  preview.className = "detail-lightbox-image";
  content.append(preview);
  host.append(source, overlay, content);
  return { source, overlay, content, preview };
}

async function finishAnimations() {
  for (const animation of [...pending]) animation.finish();
  await Promise.resolve();
  await Promise.resolve();
}

describe("海报开合动画", () => {
  beforeEach(() => {
    reduced = false;
    pending.splice(0);
    host = document.createElement("div");
    document.body.append(host);
    window.matchMedia = (() => ({
      matches: reduced,
    })) as unknown as typeof window.matchMedia;
    Object.defineProperty(Element.prototype, "animate", {
      configurable: true,
      value(
        this: Element,
        keyframes: Keyframe[],
        options: KeyframeAnimationOptions,
      ) {
        let finish = () => {};
        let reject = (_error: Error) => {};
        const finished = new Promise<void>((resolve, fail) => {
          finish = resolve;
          reject = fail;
        });
        const record: PendingAnimation = {
          element: this,
          keyframes,
          options,
          finish,
          cancelled: false,
        };
        pending.push(record);
        return {
          finished,
          cancel: () => {
            record.cancelled = true;
            reject(new Error("ANIMATION_CANCELLED"));
          },
        } as unknown as Animation;
      },
    });
  });
  afterEach(() => {
    if (originalAnimate)
      Object.defineProperty(Element.prototype, "animate", originalAnimate);
    else Reflect.deleteProperty(Element.prototype, "animate");
    window.matchMedia = originalMatchMedia;
    host.remove();
    document
      .querySelectorAll('[data-poster-motion="true"]')
      .forEach((element) => {
        element.remove();
      });
  });

  test("从真实封面放大至预览位置，关闭还原到封面后才回调", async () => {
    const { source, content } = fixture();
    const motion = createPosterMotion(source);
    motion.open(content);
    expect(pending[0]?.keyframes).toEqual([
      { left: "12px", top: "24px", width: "100px", height: "150px" },
      { left: "300px", top: "100px", width: "400px", height: "600px" },
    ]);
    expect(pending[0]?.options.duration).toBe(460);
    expect(source.style.visibility).toBe("hidden");
    await finishAnimations();
    expect(document.querySelector('[data-poster-motion="true"]')).toBeNull();
    let closed = false;
    motion.close(content, () => {
      closed = true;
    });
    expect(closed).toBe(false);
    expect(pending[3]?.keyframes).toEqual([
      { left: "300px", top: "100px", width: "400px", height: "600px" },
      { left: "12px", top: "24px", width: "100px", height: "150px" },
    ]);
    expect(pending[3]?.options.duration).toBe(320);
    await finishAnimations();
    expect(closed).toBe(true);
    expect(source.style.visibility).toBe("");
    expect(document.querySelector('[data-poster-motion="true"]')).toBeNull();
  });

  test("会话中止或卸载取消飞行层，迟到完成不会留下图片或回调", async () => {
    const { source, content } = fixture();
    const motion = createPosterMotion(source);
    motion.open(content);
    motion.cancel();
    await finishAnimations();
    expect(pending.every((animation) => animation.cancelled)).toBe(true);
    expect(source.style.visibility).toBe("");
    expect(document.querySelector('[data-poster-motion="true"]')).toBeNull();
  });

  test("缩回结束到弹窗卸载之间仍保持原图隐藏和透明终态，不闪回大图或遮罩", async () => {
    const { source, overlay, content, preview } = fixture();
    const motion = createPosterMotion(source);
    motion.open(content);
    expect(overlay.style.animation).toBe("none");
    await finishAnimations();
    let closed = false;
    motion.close(content, () => {
      closed = true;
      expect(content.isConnected).toBe(true);
      expect(preview.style.visibility).toBe("hidden");
      expect(overlay.style.animation).toBe("none");
      expect(pending.slice(3).every((animation) => !animation.cancelled)).toBe(
        true,
      );
    });
    await finishAnimations();
    expect(closed).toBe(true);
    expect(source.style.visibility).toBe("");
    expect(document.querySelector('[data-poster-motion="true"]')).toBeNull();
    content.remove();
    motion.cancel();
    expect(pending.every((animation) => animation.cancelled)).toBe(true);
    expect(preview.style.visibility).toBe("");
  });

  test("关闭途中收到轮播切换不能取消关闭回调或锁住弹窗", async () => {
    const { source, content } = fixture();
    const motion = createPosterMotion(source);
    motion.open(content);
    await finishAnimations();
    let closed = 0;
    motion.close(content, () => {
      closed++;
    });
    expect(motion.interrupt()).toBe(false);
    await finishAnimations();
    expect(closed).toBe(1);
    expect(source.style.visibility).toBe("");
    expect(document.querySelector('[data-poster-motion="true"]')).toBeNull();
  });

  test("原图在开场动画完成前失败并被移除时不残留飞行封面", async () => {
    const { source, content, preview } = fixture();
    Object.defineProperty(preview, "complete", {
      configurable: true,
      value: false,
    });
    const motion = createPosterMotion(source);
    motion.open(content);
    preview.dispatchEvent(new Event("error"));
    preview.remove();
    await finishAnimations();
    expect(document.querySelector('[data-poster-motion="true"]')).toBeNull();
    let closed = false;
    motion.close(content, () => {
      closed = true;
    });
    expect(closed).toBe(true);
    expect(source.style.visibility).toBe("");
  });

  test("减弱动态或不支持动画时直接开关，不创建独立图片层", () => {
    const { source, content } = fixture();
    reduced = true;
    const motion = createPosterMotion(source);
    motion.open(content);
    let closed = false;
    motion.close(content, () => {
      closed = true;
    });
    expect(closed).toBe(true);
    expect(pending).toHaveLength(0);
    expect(document.querySelector('[data-poster-motion="true"]')).toBeNull();
    reduced = false;
    Object.defineProperty(content, "animate", { value: undefined });
    const unsupported = createPosterMotion(source);
    unsupported.open(content);
    unsupported.close(content, () => {
      closed = false;
    });
    expect(closed).toBe(false);
    expect(pending).toHaveLength(0);
  });

  test("异步图片插入后才测量位置，提前取消不会启动迟到动画", async () => {
    const { source, content, preview } = fixture();
    preview.remove();
    const motion = createPosterMotion(source);
    motion.open(content);
    expect(pending).toHaveLength(0);
    motion.cancel();
    content.append(preview);
    await Promise.resolve();
    expect(pending).toHaveLength(0);
    expect(document.querySelector('[data-poster-motion="true"]')).toBeNull();
  });
});
