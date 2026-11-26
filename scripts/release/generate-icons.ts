// 生成应用图标的占位图：一个 7×7 的字母点阵。
//
// 这是占位用的中性图形，不是品牌设计。换成正式图标时，直接覆盖
// `apps/desktop/src-tauri/icons/` 里的文件、换掉手机那一份，并删除本脚本即可。
//
//   bun run icons          生成 source.png 与 tray-icon.png，再交给 Tauri CLI 产出各平台尺寸；
//                          顺带生成手机上那一份（`apps/iphone/App/Assets.xcassets`）
//
// 仅依赖 Bun 与 Node 内置模块，不需要图像库。

import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

const here = dirname(fileURLToPath(import.meta.url));
const iconDir = join(here, "../../apps/desktop/src-tauri/icons");
const phoneIcon = join(
  here,
  "../../apps/iphone/App/Assets.xcassets/AppIcon.appiconset/icon.png",
);

const GLYPH = [
  "..###..",
  ".#...#.",
  "#.....#",
  "#.....#",
  "#######",
  "#.....#",
  "#.....#",
];

const SOURCE_SIZE = 1024;
const TRAY_SIZE = 44;

// 应用图标的底色与字色。中性深色，方便在浅色与深色桌面上都看得清。
const BACKGROUND = [15, 23, 42] as const;
const FOREGROUND = [248, 250, 252] as const;

type Rgba = readonly [number, number, number, number];
type Pixel = (x: number, y: number) => Rgba;

function crc32(bytes: Uint8Array): number {
  let crc = -1;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
    }
  }
  return (crc ^ -1) >>> 0;
}

function chunk(type: string, data: Uint8Array): Buffer {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), Buffer.from(data)]);
  const checksum = Buffer.alloc(4);
  checksum.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, checksum]);
}

// `opaque` 为真时只写三个通道：手机上的图标不许带透明通道。
function encodePng(size: number, pixel: Pixel, opaque = false): Buffer {
  const channels = opaque ? 3 : 4;
  const stride = size * channels + 1;
  const raw = Buffer.alloc(stride * size);
  for (let y = 0; y < size; y += 1) {
    raw[y * stride] = 0;
    for (let x = 0; x < size; x += 1) {
      const [red, green, blue, alpha] = pixel(x, y);
      const offset = y * stride + 1 + x * channels;
      raw[offset] = red;
      raw[offset + 1] = green;
      raw[offset + 2] = blue;
      if (!opaque) raw[offset + 3] = alpha;
    }
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0);
  header.writeUInt32BE(size, 4);
  header[8] = 8;
  header[9] = opaque ? 2 : 6;
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", new Uint8Array()),
  ]);
}

// 以 4×4 超采样估算一个像素里图形覆盖的比例，得到 0 到 1 之间的覆盖率。
function coverage(
  x: number,
  y: number,
  inside: (px: number, py: number) => boolean,
): number {
  const samples = 4;
  let hits = 0;
  for (let sy = 0; sy < samples; sy += 1) {
    for (let sx = 0; sx < samples; sx += 1) {
      if (inside(x + (sx + 0.5) / samples, y + (sy + 0.5) / samples)) hits += 1;
    }
  }
  return hits / (samples * samples);
}

function inGlyph(
  px: number,
  py: number,
  origin: number,
  cell: number,
): boolean {
  const column = Math.floor((px - origin) / cell);
  const row = Math.floor((py - origin) / cell);
  if (
    row < 0 ||
    row >= GLYPH.length ||
    column < 0 ||
    column >= GLYPH[row]?.length
  ) {
    return false;
  }
  return GLYPH[row]?.[column] === "#";
}

function inRoundedSquare(
  px: number,
  py: number,
  size: number,
  radius: number,
): boolean {
  if (px < 0 || py < 0 || px >= size || py >= size) return false;
  const corners = [
    [radius, radius],
    [size - radius, radius],
    [radius, size - radius],
    [size - radius, size - radius],
  ];
  for (const [cx, cy] of corners) {
    // 只在圆心所在的那个角落附近才需要判断是否落在圆外。
    const nearCorner =
      px < cx === (cx === radius) && py < cy === (cy === radius);
    if (nearCorner && (px - cx) ** 2 + (py - cy) ** 2 > radius ** 2)
      return false;
  }
  return true;
}

function sourceIcon(): Buffer {
  const size = SOURCE_SIZE;
  const cell = Math.round((size * 0.56) / GLYPH.length);
  const origin = Math.round((size - cell * GLYPH.length) / 2);
  const radius = Math.round(size * 0.22);
  return encodePng(size, (x, y) => {
    const plate = coverage(x, y, (px, py) =>
      inRoundedSquare(px, py, size, radius),
    );
    const mark = coverage(x, y, (px, py) => inGlyph(px, py, origin, cell));
    const blend = FOREGROUND.map(
      (channel, index) => channel * mark + BACKGROUND[index] * (1 - mark),
    );
    return [
      Math.round(blend[0] ?? 0),
      Math.round(blend[1] ?? 0),
      Math.round(blend[2] ?? 0),
      Math.round(plate * 255),
    ];
  });
}

// 手机上的那一份：同一条字母点阵，铺满整个正方形。iOS 自己给图标切圆角，
// 所以这里不做圆角，也不留透明通道（App Store 的要求）。
function phoneIconImage(): Buffer {
  const size = SOURCE_SIZE;
  const cell = Math.round((size * 0.56) / GLYPH.length);
  const origin = Math.round((size - cell * GLYPH.length) / 2);
  return encodePng(
    size,
    (x, y) => {
      const mark = coverage(x, y, (px, py) => inGlyph(px, py, origin, cell));
      const blend = FOREGROUND.map(
        (channel, index) => channel * mark + BACKGROUND[index] * (1 - mark),
      );
      return [
        Math.round(blend[0] ?? 0),
        Math.round(blend[1] ?? 0),
        Math.round(blend[2] ?? 0),
        255,
      ];
    },
    true,
  );
}

// 菜单栏模板图：纯黑加透明度，由系统按深浅外观着色。
function trayIcon(): Buffer {
  const cell = 6;
  const origin = Math.round((TRAY_SIZE - cell * GLYPH.length) / 2);
  return encodePng(TRAY_SIZE, (x, y) => [
    0,
    0,
    0,
    Math.round(255 * coverage(x, y, (px, py) => inGlyph(px, py, origin, cell))),
  ]);
}

function main(): void {
  mkdirSync(iconDir, { recursive: true });
  writeFileSync(join(iconDir, "source.png"), sourceIcon());
  writeFileSync(join(iconDir, "tray-icon.png"), trayIcon());
  mkdirSync(dirname(phoneIcon), { recursive: true });
  writeFileSync(phoneIcon, phoneIconImage());
  console.log(
    `已生成 ${join(iconDir, "source.png")}、tray-icon.png 与手机上的图标`,
  );
}

if (import.meta.main) {
  main();
  // 保留给后续用 Tauri CLI 生成各平台尺寸的入口；source.png 用完即可删除。
  if (process.argv.includes("--clean-source")) {
    rmSync(join(iconDir, "source.png"), { force: true });
  }
}
