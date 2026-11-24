// 验收脚本里不碰进程与界面的那部分：解析参数、认标记行、找落盘的文件。
//
// 单独成文件是为了能测试：入口脚本一被导入就开始干活，没法在测试里导入它。

import { createHash, randomBytes } from "node:crypto";
import { createReadStream } from "node:fs";
import { readdir, stat } from "node:fs/promises";
import { join } from "node:path";

export interface Options {
  command: "start" | "stop";
  /** 在这台电脑上登录用的账户；还没有就现建一个。 */
  account: string;
  /** 每一步最多等多少秒。 */
  timeout: number;
}

export const DEFAULT_ACCOUNT = "dev";
export const DEFAULT_TIMEOUT_SECONDS = 600;

export const USAGE = [
  "用法：",
  "  bun run acceptance [--account <用户名>] [--timeout <秒>]   起环境、等你登录、自动核对",
  "  bun run acceptance stop                                  停掉替身设备与本脚本起的服务端",
].join("\n");

/** 命令行参数；有不认识的、或者值不合法的，返回一段说明。 */
export function parseArguments(argv: readonly string[]): Options | string {
  if (argv[0] === "stop") {
    return argv.length === 1
      ? {
          command: "stop",
          account: DEFAULT_ACCOUNT,
          timeout: DEFAULT_TIMEOUT_SECONDS,
        }
      : `stop 后面不接别的参数。\n${USAGE}`;
  }

  const options: Options = {
    command: "start",
    account: DEFAULT_ACCOUNT,
    timeout: DEFAULT_TIMEOUT_SECONDS,
  };
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (value === undefined) return `${flag} 后面要跟一个值。\n${USAGE}`;
    if (flag === "--account") {
      // 与服务端建账户时的规则一致；大小写不分，一律按小写记。
      if (!/^[a-z\d][a-z\d_.-]{2,31}$/i.test(value)) {
        return `用户名是 3 到 32 个字符：小写字母、数字、下划线、点或横线，以字母或数字开头。\n${USAGE}`;
      }
      options.account = value.toLowerCase();
    } else if (flag === "--timeout") {
      if (!/^[1-9]\d{0,4}$/.test(value) || Number(value) > 3600) {
        return `--timeout 是 1 到 3600 之间的秒数。\n${USAGE}`;
      }
      options.timeout = Number(value);
    } else {
      return `不认识的参数：${flag}\n${USAGE}`;
    }
  }
  return options;
}

/**
 * 替身设备输出里给脚本读的行：行首是标记，后面用制表符分开字段。
 * 其余的行是给人看的，措辞随时会改，所以只认行首的标记，并且要求标记后面紧跟制表符或行尾。
 */
export function markerLines(text: string, marker: string): string[][] {
  const lines: string[][] = [];
  for (const line of text.split("\n")) {
    if (line === marker) {
      lines.push([]);
    } else if (line.startsWith(`${marker}\t`)) {
      lines.push(line.slice(marker.length + 1).split("\t"));
    }
  }
  return lines;
}

/** 扩展名超过这个长度（字节）就不当扩展名看了，与接收时的规则一致。 */
const MAX_EXTENSION_BYTES = 64;

export interface FileName {
  stem: string;
  extension: string;
}

/**
 * 把扩展名与主干分开：开头的点不算扩展名（`.profile` 整个是主干），
 * `.tar.gz` 这类压缩包的双扩展名算一个整体。与接收时撞名加序号的分法一致。
 */
export function splitExtension(name: string): FileName {
  const index = name.lastIndexOf(".");
  if (
    index <= 0 ||
    Buffer.byteLength(name.slice(index)) > MAX_EXTENSION_BYTES
  ) {
    return { stem: name, extension: "" };
  }
  const packed =
    name[index - 4] === "." &&
    name.slice(index - 3, index).toLowerCase() === "tar";
  const start = packed ? index - 4 : index;
  return { stem: name.slice(0, start), extension: name.slice(start) };
}

/** 同名文件已经存在时用的名字：`报告 (2).pdf`。 */
export function numberedName(name: string, number: number): string {
  const { stem, extension } = splitExtension(name);
  return `${stem} (${number})${extension}`;
}

/**
 * `candidate` 是不是 `name` 撞名之后的样子（`报告 (2).pdf`、`报告 (3).pdf`……）。
 * 接收时同名不覆盖，后到的同名文件依次加序号。
 */
export function isNumberedName(candidate: string, name: string): boolean {
  const { stem, extension } = splitExtension(name);
  if (!candidate.startsWith(stem) || !candidate.endsWith(extension))
    return false;
  const middle = candidate.slice(
    stem.length,
    candidate.length - extension.length,
  );
  const number = /^ \((\d+)\)$/.exec(middle);
  return number !== null && Number(number[1]) >= 2;
}

export function sha256(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

export async function sha256File(path: string): Promise<string> {
  const hash = createHash("sha256");
  await new Promise<void>((resolve, reject) => {
    createReadStream(path)
      .on("data", (chunk: Buffer) => hash.update(chunk))
      .on("end", () => resolve())
      .on("error", reject);
  });
  return hash.digest("hex");
}

export interface Landed {
  path: string;
  sha256: string;
}

/**
 * 在接收目录里找刚刚落盘的那个 `name`：名字相同或者撞名后的样子，修改时间不早于 `since`，
 * 摘要对得上。目录不存在、或者没有这样的文件，都返回 `null`——调用方如实说"没找到"，不猜。
 */
export async function findLanded(
  directory: string,
  name: string,
  digest: string,
  since: number,
): Promise<Landed | null> {
  const entries = await readdir(directory).catch(() => []);
  for (const entry of entries) {
    if (entry !== name && !isNumberedName(entry, name)) continue;
    const path = join(directory, entry);
    const info = await stat(path).catch(() => null);
    if (info === null || !info.isFile() || info.mtimeMs < since) continue;
    const actual = await sha256File(path).catch(() => "");
    if (actual === digest) return { path, sha256: actual };
  }
  return null;
}

/** 给人看的大小：1 MB 以上用 MB，以下写字节。 */
export function formatSize(bytes: number): string {
  return bytes >= 1_048_576
    ? `${(bytes / 1_048_576).toFixed(1)} MB`
    : `${bytes} 字节`;
}

/** 生成一段随机内容，连同它的摘要。摘要之后由替身设备声明、由桌面端核对。 */
export function randomFile(bytes: number): {
  bytes: Uint8Array;
  sha256: string;
} {
  const content = randomBytes(bytes);
  return { bytes: content, sha256: sha256(content) };
}
