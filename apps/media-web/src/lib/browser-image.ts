import type { MediaImage, MediaImageVariant } from "@assemble/media";

export function browserImageSource(
  image: MediaImage,
  variant: MediaImageVariant,
) {
  if (
    !/^[A-Za-z0-9_.-]{1,200}$/.test(image.assetId) ||
    image.assetId === "." ||
    image.assetId === ".." ||
    !["original", "200", "360", "480"].includes(variant)
  )
    return null;
  return `/v1/media/assets/${encodeURIComponent(image.assetId)}/content?variant=${variant}`;
}

// 变体名不等于真实宽度：本机生成按长边缩放，旧归档则可能按宽度缩放。
// 用原生媒体条件选档，避免虚报 srcset 的宽度；阈值对应两/三/四/六列的卡片宽度。
export const posterImageSources = [
  { media: "(min-resolution: 2dppx)", variant: "480" },
  { media: "(width < 19rem)", variant: "200" },
  {
    media: "(32rem <= width < 34rem), (70rem <= width < 72rem)",
    variant: "480",
  },
] as const;
