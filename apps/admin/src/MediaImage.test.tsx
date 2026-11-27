import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { MediaImage } from "@assemble/media/components/media-image";
import type { MediaImage as Image, ImageLoader } from "@assemble/media/model";
import { act, useState } from "react";
import { button, cleanup, click, render, waitFor } from "./test-support/dom";

const first: Image = {
  assetId: "first-image",
  width: 400,
  height: 600,
  contentType: "image/webp",
};
function MissingSurface({ loader }: { loader: ImageLoader }) {
  const [image, setImage] = useState<Image | null>(null);
  return (
    <>
      <button type="button" onClick={() => setImage(first)}>
        load
      </button>
      <MediaImage
        image={image}
        imageLoader={loader}
        alt="演员"
        compact
        className="detail-actor-photo"
      />
    </>
  );
}
function SwitchingSurface({
  oldLoader,
  newLoader,
}: {
  oldLoader: ImageLoader;
  newLoader: ImageLoader;
}) {
  const [next, setNext] = useState(false);
  return (
    <>
      <button type="button" onClick={() => setNext(true)}>
        switch
      </button>
      <MediaImage
        image={first}
        imageLoader={next ? newLoader : oldLoader}
        alt="封面"
      />
    </>
  );
}
let previousUrl: string;
beforeEach(() => {
  previousUrl = window.location.href;
  (
    window as unknown as { happyDOM: { setURL: (url: string) => void } }
  ).happyDOM.setURL("http://localhost/admin/");
});
afterEach(async () => {
  await cleanup();
  (
    window as unknown as { happyDOM: { setURL: (url: string) => void } }
  ).happyDOM.setURL(previousUrl);
});

test("缺图、加载与失败保留同一个有界外框，小头像重试不会塞进额外文案", async () => {
  let finish: ((value: string) => void) | undefined;
  const loader = mock<ImageLoader>(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const host = await render(<MissingSurface loader={loader} />);
  const frame = host.querySelector(".media-image-frame");
  expect(frame?.getAttribute("data-image-state")).toBe("missing");
  await click(button(host, "load"));
  expect(host.querySelector(".media-image-frame")).toBe(frame);
  expect(frame?.getAttribute("data-image-state")).toBe("loading");
  await act(async () => {
    finish?.("/image.webp");
  });
  await waitFor(() => Boolean(frame?.querySelector("img")), "加载图片");
  const image = frame?.querySelector("img");
  await act(async () => {
    image?.dispatchEvent(new Event("error"));
  });
  expect(host.querySelector(".media-image-frame")).toBe(frame);
  expect(frame?.getAttribute("data-image-state")).toBe("failed");
  expect(frame?.classList.contains("detail-actor-photo")).toBe(true);
  expect(frame?.querySelector("span")?.classList.contains("sr-only")).toBe(
    true,
  );
  await click(button(host, "重新加载图片：演员"));
  expect(loader).toHaveBeenCalledTimes(2);
  expect(frame?.getAttribute("data-image-state")).toBe("loading");
});

test("图片或加载器切换取消旧请求，迟到结果不能恢复旧图片", async () => {
  let finishFirst: ((value: string) => void) | undefined;
  let finishSecond: ((value: string) => void) | undefined;
  let oldSignal: AbortSignal | undefined;
  const oldLoader: ImageLoader = (_image, _variant, signal) => {
    oldSignal = signal;
    return new Promise((resolve) => {
      finishFirst = resolve;
    });
  };
  const newLoader: ImageLoader = () =>
    new Promise((resolve) => {
      finishSecond = resolve;
    });
  const host = await render(
    <SwitchingSurface oldLoader={oldLoader} newLoader={newLoader} />,
  );
  await click(button(host, "switch"));
  expect(oldSignal?.aborted).toBe(true);
  await act(async () => finishSecond?.("/new-image.webp"));
  await waitFor(
    () => host.querySelector("img")?.getAttribute("src") === "/new-image.webp",
    "新图片",
  );
  await act(async () => finishFirst?.("/old-image.webp"));
  expect(host.querySelector("img")?.getAttribute("src")).toBe(
    "/new-image.webp",
  );
});
