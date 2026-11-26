import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { setLanguage } from "@assemble/display";
import {
  browserImageLoader,
  type ImageLoader,
  type MediaCandidate,
  type MediaImage,
} from "@assemble/media";
import { act } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { browserImageSource } from "../lib/browser-image";
import { button, cleanup, click, render } from "../test-support/dom";
import { MediaImageView, PosterCard } from "./poster";

const image: MediaImage = {
  assetId: "asset-poster-1",
  width: 1000,
  height: 1500,
  contentType: "image/jpeg",
};
const item: MediaCandidate = {
  identity: { provider: "tmdb", mediaType: "movie", externalId: "42" },
  title: "海报测试作品",
  originalTitle: "Poster fixture",
  year: 2026,
  overview: "海报简介",
  poster: image,
};

let previousUrl = "about:blank";
beforeEach(async () => {
  previousUrl = window.location.href;
  (
    window as unknown as { happyDOM: { setURL: (url: string) => void } }
  ).happyDOM.setURL("http://localhost/movies/");
  await setLanguage("zh-CN");
});
afterEach(async () => {
  await cleanup();
  (
    window as unknown as { happyDOM: { setURL: (url: string) => void } }
  ).happyDOM.setURL(previousUrl);
});

test("browser images appear in the first render without waiting for an effect", () => {
  const markup = renderToStaticMarkup(
    <MediaImageView
      image={image}
      imageLoader={browserImageLoader}
      alt="作品海报"
      priority
    />,
  );
  expect(markup).toContain("<img");
  expect(markup).toContain(
    'src="/v1/media/assets/asset-poster-1/content?variant=360"',
  );
  expect(markup).toContain('width="1000" height="1500"');
  expect(markup).toContain('loading="eager"');
  expect(markup).toContain('fetchPriority="high"');
  expect(markup).not.toContain('data-slot="loading-state"');
});

test("cards use native responsive picture sources without claiming variant widths", async () => {
  const host = await render(
    <PosterCard item={item} imageLoader={browserImageLoader} />,
  );
  const picture = host.querySelector("picture");
  expect(picture).not.toBeNull();
  const sources = [...(picture?.querySelectorAll("source") ?? [])];
  expect(sources.map((source) => source.getAttribute("srcset"))).toEqual([
    "/v1/media/assets/asset-poster-1/content?variant=480",
    "/v1/media/assets/asset-poster-1/content?variant=200",
    "/v1/media/assets/asset-poster-1/content?variant=480",
  ]);
  expect(sources[0]?.media).toBe("(min-resolution: 2dppx)");
  expect(sources[1]?.media).toBe("(width < 19rem)");
  expect(sources[2]?.media).toContain("(32rem <= width < 34rem)");
  expect(sources[2]?.media).toContain("(70rem <= width < 72rem)");
  const poster = picture?.querySelector("img");
  expect(poster?.getAttribute("loading")).toBe("lazy");
  expect(poster?.getAttribute("fetchpriority")).toBe("low");
  expect(poster?.getAttribute("src")).toEndWith("?variant=360");
  expect(poster?.getAttribute("alt")).toBe("");
  expect(host.querySelector("a")?.getAttribute("href")).toBe(
    "/movies/catalog/tmdb/movie/42",
  );
});

test("original preview images retain the original variant and dimensions", () => {
  const markup = renderToStaticMarkup(
    <MediaImageView
      image={image}
      imageLoader={browserImageLoader}
      alt="原图"
      variant="original"
    />,
  );
  expect(markup).toContain("?variant=original");
  expect(markup).toContain('width="1000" height="1500"');
  expect(markup).not.toContain("<picture");
});

test("browser image URLs reject paths and always use the fixed same-origin route", () => {
  for (const assetId of [
    "..",
    ".",
    "../secret",
    "https://example.com",
    "x?key=y",
  ])
    expect(browserImageSource({ ...image, assetId }, "360")).toBeNull();
  expect(browserImageSource(image, "480")).toBe(
    "/v1/media/assets/asset-poster-1/content?variant=480",
  );
});

test("browser image failures can be retried with a new image element", async () => {
  const host = await render(
    <MediaImageView
      image={image}
      imageLoader={browserImageLoader}
      alt="作品海报"
      responsivePoster
    />,
  );
  const failed = host.querySelector("img");
  await act(async () => failed?.dispatchEvent(new Event("error")));
  expect(host.querySelector("img")).toBeNull();
  await click(button(host, "重新载入图片"));
  const retried = host.querySelector("img");
  expect(retried).not.toBeNull();
  expect(retried).not.toBe(failed);
  expect(retried?.getAttribute("src")).toEndWith("?variant=360");
});

test("custom image loaders keep async loading, retry, and their returned source", async () => {
  let attempts = 0;
  const loader = mock<ImageLoader>(async () => {
    attempts += 1;
    if (attempts === 1) throw new Error("IMAGE_UNAVAILABLE");
    return "data:image/png;base64,dGVzdA==";
  });
  const initial = renderToStaticMarkup(
    <MediaImageView image={image} imageLoader={loader} alt="自定义海报" />,
  );
  expect(initial).toContain('data-slot="loading-state"');
  expect(loader).not.toHaveBeenCalled();
  const host = await render(
    <MediaImageView
      image={image}
      imageLoader={loader}
      alt="自定义海报"
      responsivePoster
    />,
  );
  await click(button(host, "重新载入图片"));
  expect(loader).toHaveBeenCalledTimes(2);
  expect(host.querySelector("img")?.getAttribute("src")).toBe(
    "data:image/png;base64,dGVzdA==",
  );
  expect(host.querySelector("picture")).toBeNull();
  expect(loader.mock.calls[0]?.[2]).toBeInstanceOf(AbortSignal);
});
