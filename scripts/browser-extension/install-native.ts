// 只有用户显式运行本安装器时才登记浏览器宿主；构建和自动验证均不修改浏览器设置。

import { createHash } from "node:crypto";
import { chmod, copyFile, mkdir } from "node:fs/promises";
import { homedir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const root = fileURLToPath(new URL("../..", import.meta.url));
const { values } = parseArgs({
  args: Bun.argv.slice(2),
  options: {
    desktop: { type: "string" },
    "app-id": { type: "string", default: "com.puzzlefuzzy.assemble" },
    host: { type: "string" },
    "dry-run": { type: "boolean", default: false },
  },
});
if (
  !values.desktop ||
  !isAbsolute(values.desktop) ||
  !(await Bun.file(values.desktop).exists())
)
  throw new Error(
    "Provide --desktop with the matching Assemble executable's absolute path",
  );
if (!/^[A-Za-z0-9._-]{1,200}$/.test(values["app-id"] ?? ""))
  throw new Error("Invalid application identifier");
if (!["win32", "darwin"].includes(process.platform))
  throw new Error("The browser connector supports Windows and macOS");
const suffix = process.platform === "win32" ? ".exe" : "";
const source = resolve(
  values.host ?? join(root, `target/release/assemble-browser-host${suffix}`),
);
const bytes = await Bun.file(source).arrayBuffer();
const digest = createHash("sha256")
  .update(new Uint8Array(bytes))
  .digest("hex")
  .slice(0, 16);
const manifest = await Bun.file(
  join(root, "apps/browser-extension/manifest.json"),
).json();
const id = createHash("sha256")
  .update(Buffer.from(manifest.key, "base64"))
  .digest("hex")
  .slice(0, 32)
  .replace(/[0-9a-f]/g, (v) =>
    String.fromCharCode(97 + Number.parseInt(v, 16)),
  );
const data =
  process.platform === "win32"
    ? process.env.LOCALAPPDATA
    : join(homedir(), "Library/Application Support");
if (!data) throw new Error("User data directory unavailable");
const folder = join(data, "Assemble/BrowserConnector", digest);
const target = join(folder, `assemble-browser-host${suffix}`);
const hostManifest = {
  name: "com.puzzlefuzzy.assemble",
  description: "Assemble browser downloads",
  path: target,
  type: "stdio",
  allowed_origins: [`chrome-extension://${id}/`],
};
const hostManifestPath =
  process.platform === "win32"
    ? join(folder, "com.puzzlefuzzy.assemble.json")
    : join(
        homedir(),
        "Library/Application Support/Google/Chrome/NativeMessagingHosts/com.puzzlefuzzy.assemble.json",
      );
if (values["dry-run"]) {
  console.log(
    JSON.stringify(
      {
        extensionId: id,
        desktop: values.desktop,
        appId: values["app-id"],
        hostManifest: hostManifestPath,
        host: target,
      },
      null,
      2,
    ),
  );
  process.exit(0);
}
await mkdir(folder, { recursive: true, mode: 0o700 });
await copyFile(source, target);
if (process.platform !== "win32") await chmod(target, 0o700);
await Bun.write(
  join(folder, "assemble-browser-host.json"),
  JSON.stringify(
    { extensionId: id, desktopPath: values.desktop, appId: values["app-id"] },
    null,
    2,
  ),
);
await mkdir(resolve(hostManifestPath, ".."), { recursive: true, mode: 0o700 });
await Bun.write(hostManifestPath, JSON.stringify(hostManifest, null, 2));
if (process.platform === "win32") {
  const child = Bun.spawn(
    [
      "reg.exe",
      "add",
      "HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\com.puzzlefuzzy.assemble",
      "/ve",
      "/t",
      "REG_SZ",
      "/d",
      hostManifestPath,
      "/f",
    ],
    { stdout: "ignore", stderr: "ignore" },
  );
  if ((await child.exited) !== 0)
    throw new Error("Native messaging registration failed");
}
console.log(`Assemble browser connector registered for ${id}`);
