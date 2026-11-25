import { createHash } from "node:crypto";
import { copyFile, cp, mkdir } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const child = Bun.spawn(["bunx", "vite", "build"], {
  cwd: root,
  stdout: "inherit",
  stderr: "inherit",
});
if ((await child.exited) !== 0) process.exit(1);
for (const entry of ["background", "detector"]) {
  const result = await Bun.build({
    entrypoints: [join(root, `src/${entry}.ts`)],
    outdir: join(root, "dist"),
    target: "browser",
    format: entry === "detector" ? "iife" : "esm",
    minify: true,
  });
  if (!result.success) throw new Error(`extension ${entry} build failed`);
}
const icons = join(root, "dist/icons");
await cp(join(root, "_locales"), join(root, "dist/_locales"), {
  recursive: true,
});
await mkdir(icons, { recursive: true });
for (const size of [32, 128])
  await copyFile(
    join(root, `../desktop/src-tauri/icons/${size}x${size}.png`),
    join(icons, `icon-${size}.png`),
  );
const manifest = await Bun.file(join(root, "manifest.json")).json();
await Bun.write(
  join(root, "dist/manifest.json"),
  JSON.stringify(manifest, null, 2),
);
const id = createHash("sha256")
  .update(Buffer.from(manifest.key, "base64"))
  .digest("hex")
  .slice(0, 32)
  .replace(/[0-9a-f]/g, (value) =>
    String.fromCharCode(97 + Number.parseInt(value, 16)),
  );
console.log(`Chrome extension built: ${join(root, "dist")} (${id})`);
