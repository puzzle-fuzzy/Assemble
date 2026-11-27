// 维护者本地执行：先落盘和核验，再替换清单；服务端始终只读。
import { createHash } from "node:crypto";
import {
  copyFile,
  link,
  lstat,
  mkdir,
  open,
  rename,
  rmdir,
  unlink,
} from "node:fs/promises";
import {
  basename,
  extname,
  isAbsolute,
  join,
  relative,
  resolve,
} from "node:path";
import {
  type Architecture,
  type Artifact,
  MAX_ARTIFACT_BYTES,
  MAX_MANIFEST_BYTES,
  type Manifest,
  type Platform,
  parseManifest,
  validArtifact,
} from "../../apps/releases-web/src/lib/catalog";

async function regular(path: string) {
  const info = await lstat(path);
  if (!info.isFile() || info.isSymbolicLink())
    throw new Error("RELEASE_FILE_REQUIRED");
  return info;
}
async function digest(path: string) {
  const hash = createHash("sha256");
  const file = await open(path, "r");
  try {
    const buffer = Buffer.alloc(64 * 1024);
    while (true) {
      const { bytesRead } = await file.read(buffer);
      if (!bytesRead) break;
      hash.update(buffer.subarray(0, bytesRead));
    }
  } finally {
    await file.close();
  }
  return hash.digest("hex");
}
async function manifest(directory: string): Promise<Manifest> {
  const path = join(directory, "manifest.json");
  try {
    if ((await regular(path)).size > MAX_MANIFEST_BYTES)
      throw new Error("RELEASE_MANIFEST_TOO_LARGE");
    const file = await open(path, "r");
    try {
      const buffer = Buffer.alloc(MAX_MANIFEST_BYTES + 1);
      let length = 0;
      while (length < buffer.length) {
        const { bytesRead } = await file.read(
          buffer,
          length,
          buffer.length - length,
        );
        if (!bytesRead) break;
        length += bytesRead;
      }
      if (length > MAX_MANIFEST_BYTES)
        throw new Error("RELEASE_MANIFEST_TOO_LARGE");
      return parseManifest(
        JSON.parse(
          new TextDecoder("utf-8", { fatal: true }).decode(
            buffer.subarray(0, length),
          ),
        ),
      );
    } finally {
      // 流到达 EOF 不保证异步关闭已完成；Windows 下必须先关闭读句柄才能原子替换清单。
      await file.close();
    }
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT")
      return { schemaVersion: 1, artifacts: [] };
    throw new Error("INVALID_RELEASE_MANIFEST");
  }
}
async function objectDirectory(directory: string) {
  const objects = join(directory, "objects");
  await mkdir(objects, { recursive: true });
  const info = await lstat(objects);
  if (!info.isDirectory() || info.isSymbolicLink())
    throw new Error("RELEASE_DIRECTORY_REQUIRED");
  return objects;
}
async function packageFormat(path: string, extension: string) {
  const file = await open(path, "r");
  try {
    const bytes = Buffer.alloc(8);
    const size = (await file.stat()).size;
    await file.read(bytes, 0, 8, 0);
    if (extension === ".exe" && bytes.subarray(0, 2).toString() === "MZ")
      return;
    if (extension === ".msi" && bytes.toString("hex") === "d0cf11e0a1b11ae1")
      return;
    if (extension === ".pkg" && bytes.subarray(0, 4).toString() === "xar!")
      return;
    if (
      [".apk", ".ipa"].includes(extension) &&
      bytes.subarray(0, 4).equals(Buffer.from([80, 75, 3, 4]))
    )
      return;
    if (extension === ".dmg" && size >= 512) {
      await file.read(bytes, 0, 4, size - 512);
      if (bytes.subarray(0, 4).toString() === "koly") return;
    }
    throw new Error("RELEASE_PACKAGE_FORMAT_MISMATCH");
  } finally {
    await file.close();
  }
}
export interface PublishOptions {
  directory: string;
  source: string;
  platform: Platform;
  architecture: Architecture;
  version: string;
}
export async function publishArtifact(
  options: PublishOptions,
): Promise<Artifact> {
  const directory = resolve(options.directory);
  const source = resolve(options.source);
  const location = relative(directory, source);
  if (
    !isAbsolute(location) &&
    location !== ".." &&
    !location.startsWith(`..${process.platform === "win32" ? "\\" : "/"}`)
  )
    throw new Error("RELEASE_SOURCE_INSIDE_STORE");
  const info = await regular(source);
  if (!info.size || info.size > MAX_ARTIFACT_BYTES)
    throw new Error("RELEASE_SIZE_INVALID");
  const extension = extname(basename(source)).toLowerCase();
  const artifact: Artifact = {
    platform: options.platform,
    architecture: options.architecture,
    version: options.version,
    fileName: `Assemble-${options.version}-${options.platform}-${options.architecture}${extension}`,
    sha256: "0".repeat(64),
    size: info.size,
    publishedAt: new Date().toISOString(),
  };
  if (!validArtifact(artifact)) throw new Error("INVALID_RELEASE_ARTIFACT");
  await packageFormat(source, extension);
  await mkdir(directory, { recursive: true });
  if ((await lstat(directory)).isSymbolicLink())
    throw new Error("RELEASE_DIRECTORY_REQUIRED");
  const lock = join(directory, ".publish-lock");
  try {
    await mkdir(lock);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "EEXIST")
      throw new Error("RELEASE_PUBLISH_IN_PROGRESS");
    throw error;
  }
  let temporary: string | null = null;
  let manifestTemporary: string | null = null;
  try {
    const current = await manifest(directory);
    const objects = await objectDirectory(directory);
    temporary = join(objects, `.stage-${crypto.randomUUID()}`);
    await copyFile(source, temporary, 1);
    const staged = await regular(temporary);
    artifact.sha256 = await digest(temporary);
    if (staged.size !== info.size || (await digest(source)) !== artifact.sha256)
      throw new Error("RELEASE_SOURCE_CHANGED");
    await packageFormat(temporary, extension);
    const existing = current.artifacts.find(
      (item) =>
        item.sha256 === artifact.sha256 ||
        (item.platform === artifact.platform &&
          item.architecture === artifact.architecture &&
          item.version === artifact.version),
    );
    if (existing) {
      if (
        existing.sha256 !== artifact.sha256 ||
        existing.platform !== artifact.platform ||
        existing.architecture !== artifact.architecture ||
        existing.version !== artifact.version ||
        existing.fileName !== artifact.fileName ||
        existing.size !== artifact.size
      )
        throw new Error("RELEASE_IDENTITY_CONFLICT");
      const target = join(objects, existing.sha256);
      if (
        (await regular(target)).size !== existing.size ||
        (await digest(target)) !== existing.sha256
      )
        throw new Error("RELEASE_OBJECT_CORRUPT");
      return existing;
    }
    const next = parseManifest({
      schemaVersion: 1,
      artifacts: [...current.artifacts, artifact],
    });
    const encoded = `${JSON.stringify(next, null, 2)}\n`;
    if (Buffer.byteLength(encoded) > MAX_MANIFEST_BYTES)
      throw new Error("RELEASE_MANIFEST_TOO_LARGE");
    // Windows 的 fsync 需要可写句柄；只刷写自己的暂存文件，不打开已发布对象写入。
    const stagedObject = await open(temporary, "r+");
    try {
      await stagedObject.sync();
    } finally {
      await stagedObject.close();
    }
    const target = join(objects, artifact.sha256);
    try {
      // 同目录硬链接只发布已核验的完整对象；目标存在时绝不覆盖。
      await link(temporary, target);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
      if (
        (await regular(target)).size !== artifact.size ||
        (await digest(target)) !== artifact.sha256
      )
        throw new Error("RELEASE_OBJECT_CORRUPT");
    }
    manifestTemporary = join(directory, `.manifest-${crypto.randomUUID()}`);
    const output = await open(manifestTemporary, "wx");
    try {
      await output.writeFile(encoded, "utf8");
      await output.sync();
    } finally {
      await output.close();
    }
    await rename(manifestTemporary, join(directory, "manifest.json"));
    manifestTemporary = null;
    return artifact;
  } finally {
    if (temporary) await unlink(temporary).catch(() => {});
    if (manifestTemporary) await unlink(manifestTemporary).catch(() => {});
    await rmdir(lock);
  }
}
export async function checkReleases(directory: string): Promise<Manifest> {
  try {
    const info = await lstat(directory);
    if (!info.isDirectory() || info.isSymbolicLink())
      throw new Error("RELEASE_DIRECTORY_REQUIRED");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  const current = await manifest(resolve(directory));
  if (current.artifacts.length > 0) {
    const info = await lstat(join(directory, "objects"));
    if (!info.isDirectory() || info.isSymbolicLink())
      throw new Error("RELEASE_DIRECTORY_REQUIRED");
  }
  for (const artifact of current.artifacts) {
    const path = join(directory, "objects", artifact.sha256);
    if (
      (await regular(path)).size !== artifact.size ||
      (await digest(path)) !== artifact.sha256
    )
      throw new Error("RELEASE_OBJECT_CORRUPT");
    await packageFormat(path, extname(artifact.fileName));
  }
  return current;
}
if (import.meta.main) {
  try {
    const [command, ...args] = process.argv.slice(2);
    const flags = new Map<string, string>();
    for (let index = 0; index < args.length; index += 2) {
      const key = args[index];
      const value = args[index + 1];
      if (
        !key ||
        ![
          "--directory",
          "--source",
          "--platform",
          "--architecture",
          "--version",
        ].includes(key) ||
        !value ||
        value.startsWith("--") ||
        flags.has(key)
      )
        throw new Error("INVALID_RELEASE_ARGUMENTS");
      flags.set(key, value);
    }
    const directory = flags.get("--directory");
    if (!directory) throw new Error("RELEASE_DIRECTORY_REQUIRED");
    if (command === "check" && flags.size === 1) {
      const result = await checkReleases(directory);
      process.stdout.write(
        `Verified ${result.artifacts.length} release artifacts\n`,
      );
    } else if (command === "publish" && flags.size === 5) {
      const result = await publishArtifact({
        directory,
        source: flags.get("--source") ?? "",
        platform: flags.get("--platform") as Platform,
        architecture: flags.get("--architecture") as Architecture,
        version: flags.get("--version") ?? "",
      });
      process.stdout.write(
        `${result.platform} ${result.architecture} ${result.version} ${result.sha256} ${result.size}\n`,
      );
    } else throw new Error("INVALID_RELEASE_ARGUMENTS");
  } catch (error) {
    const message =
      error instanceof Error && /^[A-Z_]+$/.test(error.message)
        ? error.message
        : "RELEASE_OPERATION_FAILED";
    process.stderr.write(`${message}\n`);
    process.exitCode = 1;
  }
}
