import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import type { ImageLoader, MediaImage } from "@assemble/media";
import { act, useEffect, useState } from "react";
import { button, cleanup, click, render, settle } from "../test-support/dom";
import { PosterViewer } from "./detail-viewer";

const poster: MediaImage = {
  assetId: "test-cover",
  width: 500,
  height: 750,
  contentType: "image/webp",
};
const alternative = { ...poster, assetId: "test-alternative" };
const third = { ...poster, assetId: "test-third" };
const imageLoader = mock<ImageLoader>(
  async (image, variant) => `/test-images/${image.assetId}/${variant}`,
);
let previousUrl = "about:blank";

async function key(target: Element, name: string) {
  await act(async () => {
    target.dispatchEvent(
      new KeyboardEvent("keydown", { key: name, bubbles: true }),
    );
  });
  await settle();
}

function dialog() {
  const found = document.querySelector<HTMLElement>('[role="dialog"]');
  if (!found) throw new Error("PREVIEW_NOT_OPEN");
  return found;
}

async function previewImage() {
  for (let attempt = 0; attempt < 30; attempt++) {
    const image = dialog().querySelector<HTMLImageElement>(
      "img.detail-lightbox-image",
    );
    if (image) return image;
    await settle();
  }
  throw new Error("PREVIEW_IMAGE_NOT_READY");
}

function DelayedGallery({
  onReady,
}: {
  onReady: (publish: (images: MediaImage[]) => void) => void;
}) {
  const [images, setImages] = useState<MediaImage[]>([]);
  useEffect(() => onReady(setImages), [onReady]);
  return (
    <PosterViewer
      image={alternative}
      images={images}
      title="迟到图集"
      imageLoader={imageLoader}
    />
  );
}

describe("影视详情海报预览", () => {
  beforeEach(() => {
    previousUrl = window.location.href;
    (
      window as unknown as { happyDOM: { setURL: (url: string) => void } }
    ).happyDOM.setURL("http://localhost/movies/");
    setLanguage("zh-CN");
    imageLoader.mockClear();
  });
  afterEach(async () => {
    await cleanup();
    await settle();
    (
      window as unknown as { happyDOM: { setURL: (url: string) => void } }
    ).happyDOM.setURL(previousUrl);
    setLanguage("zh-CN");
  });

  test("封面点击才读取原图，轮播按钮、左右键与缩略图选择同一张图片，Escape 返回原焦点", async () => {
    const host = await render(
      <PosterViewer
        image={alternative}
        images={[poster, alternative, third]}
        title="测试电影"
        imageLoader={imageLoader}
      />,
    );
    expect(
      imageLoader.mock.calls.some(([, variant]) => variant === "original"),
    ).toBe(false);
    const trigger = button(host, "放大《测试电影》海报");
    trigger.focus();
    await click(trigger);
    expect(dialog().textContent).toContain("第 2 / 3 张");
    expect((await previewImage()).src).toContain("test-alternative/original");
    await click(button(dialog(), "下一张"));
    expect((await previewImage()).src).toContain("test-third/original");
    await key(dialog(), "ArrowRight");
    expect(dialog().textContent).toContain("第 1 / 3 张");
    await key(dialog(), "ArrowLeft");
    expect(dialog().textContent).toContain("第 3 / 3 张");
    await click(button(dialog(), "查看第 2 张海报"));
    expect(
      button(dialog(), "查看第 2 张海报").getAttribute("aria-pressed"),
    ).toBe("true");
    await key(dialog(), "Home");
    expect(dialog().textContent).toContain("第 1 / 3 张");
    await key(dialog(), "End");
    expect(dialog().textContent).toContain("第 3 / 3 张");
    await key(dialog(), "Escape");
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  test("单封面直接放大，不显示重复的图集入口或轮播控件，关闭返回原焦点", async () => {
    const host = await render(
      <PosterViewer
        image={poster}
        images={[poster]}
        title="单张海报"
        imageLoader={imageLoader}
      />,
    );
    expect(host.querySelector(".detail-gallery-open")).toBeNull();
    const trigger = button(host, "放大《单张海报》海报");
    await click(trigger);
    expect(dialog().querySelector(".detail-lightbox-controls")).toBeNull();
    expect(dialog().textContent).not.toContain("1 / 1");
    expect(dialog().querySelector(".detail-lightbox-thumbnails")).toBeNull();
    await click(button(dialog(), "关闭"));
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  test("图集慢返回且封面在中间时，已打开的预览保持同一资产", async () => {
    let publish: ((images: MediaImage[]) => void) | undefined;
    const host = await render(
      <DelayedGallery
        onReady={(value) => {
          publish = value;
        }}
      />,
    );
    await click(button(host, "放大《迟到图集》海报"));
    expect((await previewImage()).src).toContain("test-alternative/original");
    await act(async () => publish?.([poster, alternative, third]));
    await settle();
    expect((await previewImage()).src).toContain("test-alternative/original");
    expect(dialog().textContent).toContain("第 2 / 3 张");
  });

  test("图片加载失败提供重试，重新读取原图后仍保留选中序号", async () => {
    const host = await render(
      <PosterViewer
        image={poster}
        images={[poster, alternative]}
        title="重试海报"
        imageLoader={imageLoader}
      />,
    );
    await click(button(host, "浏览海报（2）"));
    await click(button(dialog(), "下一张"));
    const image = await previewImage();
    expect(image).not.toBeNull();
    await act(async () => image?.dispatchEvent(new Event("error")));
    await settle();
    expect(dialog().textContent).toContain("图片载入失败");
    await click(button(dialog(), "重新载入图片"));
    expect(dialog().textContent).toContain("第 2 / 2 张");
    expect((await previewImage()).src).toContain("test-alternative/original");
    expect(
      imageLoader.mock.calls.filter(
        ([item, variant]) =>
          item.assetId === alternative.assetId && variant === "original",
      ),
    ).toHaveLength(2);
  });

  test("会话进入重新核验时关闭 portal 内原图，过期视图不能再次打开预览", async () => {
    const session = new AbortController();
    const host = await render(
      <PosterViewer
        image={poster}
        title="私人海报"
        imageLoader={imageLoader}
        sessionSignal={session.signal}
      />,
    );
    const trigger = button(host, "放大《私人海报》海报");
    await click(trigger);
    expect(dialog()).toBeDefined();
    await act(async () => session.abort());
    await settle();
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    await click(trigger);
    expect(document.querySelector('[role="dialog"]')).toBeNull();
  });

  test("英文预览按钮、轮播控制和计数都有翻译", async () => {
    setLanguage("en");
    const host = await render(
      <PosterViewer
        image={poster}
        images={[poster, alternative]}
        title="Film"
        imageLoader={imageLoader}
      />,
    );
    await click(button(host, "Enlarge the poster for Film"));
    expect(dialog().textContent).toContain("Image 1 of 2");
    expect(button(dialog(), "Next image")).toBeDefined();
    expect(button(dialog(), "View poster 2")).toBeDefined();
  });
});
