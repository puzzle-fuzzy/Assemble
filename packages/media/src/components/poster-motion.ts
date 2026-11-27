type Rectangle = Pick<DOMRect, "left" | "top" | "width" | "height">;

function box(rect: Rectangle): Keyframe {
  return {
    left: `${rect.left}px`,
    top: `${rect.top}px`,
    width: `${rect.width}px`,
    height: `${rect.height}px`,
  };
}

function visible(rect: Rectangle) {
  return rect.width > 0 && rect.height > 0;
}

/** 独立飞行层不受 Dialog 的居中变换与滚动容器裁切，关闭仍由 Dialog 管理焦点。 */
export function createPosterMotion(source: HTMLImageElement | null) {
  let clone: HTMLImageElement | null = null;
  let target: HTMLImageElement | null = null;
  let observer: MutationObserver | null = null;
  let animations: Animation[] = [];
  let generation = 0;
  let started = false;
  let closing = false;
  let releaseImage: (() => void) | null = null;
  const sourceVisibility = source?.style.visibility ?? "";
  let targetVisibility = "";

  function permitted(element: HTMLElement) {
    return Boolean(
      source?.isConnected &&
        typeof element.animate === "function" &&
        !window.matchMedia("(prefers-reduced-motion: reduce)").matches,
    );
  }

  function clearFlight() {
    generation++;
    observer?.disconnect();
    observer = null;
    releaseImage?.();
    releaseImage = null;
    for (const animation of animations) animation.cancel();
    animations = [];
    clone?.remove();
    clone = null;
    if (target) target.style.visibility = targetVisibility;
    target = null;
  }

  function cancel() {
    clearFlight();
    if (source) source.style.visibility = sourceVisibility;
  }

  function interrupt() {
    // 用户已发起关闭时，轮播事件不能把关闭回调一起取消掉。
    if (closing) return false;
    cancel();
    return true;
  }

  function flyingImage(image: HTMLImageElement, from: Rectangle) {
    const flying = image.cloneNode(false) as HTMLImageElement;
    flying.removeAttribute("id");
    flying.removeAttribute("class");
    flying.removeAttribute("srcset");
    flying.src = image.currentSrc || image.src;
    flying.alt = "";
    flying.setAttribute("aria-hidden", "true");
    flying.dataset.posterMotion = "true";
    flying.style.cssText =
      "position:fixed;z-index:100;pointer-events:none;object-fit:contain;border-radius:10px;max-width:none;max-height:none;margin:0;visibility:visible;";
    Object.assign(flying.style, box(from));
    image.ownerDocument.body.append(flying);
    return flying;
  }

  function run(
    content: HTMLElement,
    from: Rectangle,
    to: Rectangle,
    opening: boolean,
    done: () => void,
  ) {
    if (!clone) return;
    const identity = generation;
    const duration = opening ? 460 : 320;
    const sibling = content.previousElementSibling;
    const backdrop =
      sibling instanceof HTMLElement &&
      sibling.matches('[data-slot="dialog-overlay"]')
        ? sibling
        : null;
    // 此预览已接管遮罩的淡入淡出，避免 Dialog 退出时从不透明再播放一次。
    if (backdrop) backdrop.style.animation = "none";
    try {
      animations = [
        clone.animate([box(from), box(to)], {
          duration,
          easing: opening
            ? "cubic-bezier(.2,.75,.24,1)"
            : "cubic-bezier(.4,0,.25,1)",
          fill: "forwards",
        }),
        content.animate(
          [
            { opacity: opening ? 0 : getComputedStyle(content).opacity },
            { opacity: opening ? 1 : 0 },
          ],
          { duration, easing: "ease-out", fill: "forwards" },
        ),
        ...(backdrop
          ? [
              backdrop.animate(
                [
                  { opacity: opening ? 0 : getComputedStyle(backdrop).opacity },
                  { opacity: opening ? 1 : 0 },
                ],
                { duration, easing: "ease-out", fill: "forwards" },
              ),
            ]
          : []),
      ];
      void Promise.all(animations.map((animation) => animation.finished)).then(
        () => {
          if (identity === generation) done();
        },
        () => {
          if (identity !== generation) return;
          cancel();
          if (!opening) done();
        },
      );
    } catch {
      cancel();
      if (!opening) done();
    }
  }

  function open(content: HTMLElement) {
    if (started || !source || !permitted(content)) return;
    started = true;
    function begin() {
      const image = content.querySelector<HTMLImageElement>(
        "img.detail-lightbox-image",
      );
      if (!image || !source || closing) return;
      observer?.disconnect();
      observer = null;
      const from = source.getBoundingClientRect();
      const to = image.getBoundingClientRect();
      if (!visible(from) || !visible(to)) return;
      target = image;
      targetVisibility = image.style.visibility;
      clone = flyingImage(source, from);
      source.style.visibility = "hidden";
      image.style.visibility = "hidden";
      run(content, from, to, true, () => {
        function reveal() {
          clearFlight();
        }
        if (!image.isConnected || image.complete) reveal();
        else {
          // 原图未下载完时保留放大后的封面，避免动画结束后闪出一块空白。
          image.addEventListener("load", reveal, { once: true });
          image.addEventListener("error", reveal, { once: true });
          releaseImage = () => {
            image.removeEventListener("load", reveal);
            image.removeEventListener("error", reveal);
          };
        }
      });
    }
    observer = new MutationObserver(begin);
    observer.observe(content, { childList: true, subtree: true });
    begin();
  }

  function close(content: HTMLElement | null, finish: () => void) {
    if (closing) return;
    closing = true;
    const image =
      clone ??
      content?.querySelector<HTMLImageElement>("img.detail-lightbox-image");
    if (!source || !content || !image || !permitted(content)) {
      cancel();
      finish();
      return;
    }
    const from = image.getBoundingClientRect();
    const to = source.getBoundingClientRect();
    const frame = image.cloneNode(false) as HTMLImageElement;
    clearFlight();
    if (!visible(from) || !visible(to)) {
      cancel();
      finish();
      return;
    }
    target = content.querySelector<HTMLImageElement>(
      "img.detail-lightbox-image",
    );
    targetVisibility = target?.style.visibility ?? "";
    if (target) target.style.visibility = "hidden";
    source.style.visibility = "hidden";
    clone = flyingImage(frame, from);
    run(content, from, to, false, () => {
      // Dialog 的关闭要等 React 提交；此时撤销填充动画会让原图和遮罩闪回一帧。
      // 只交还页面封面，弹窗继续保持透明终态，卸载后再由 cancel 统一释放。
      clone?.remove();
      clone = null;
      if (source) source.style.visibility = sourceVisibility;
      finish();
    });
  }

  return { open, close, cancel, interrupt };
}
