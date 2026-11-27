// 用本地的自签证书给构建出来的程序签名，让系统每次都认得它们。只在 macOS 上有意义。
//
// 没有稳定签名的程序，每重新构建一次，在系统眼里就是另一个程序：读钥匙串里存的登录状态
// 时都要再问一次用户。用同一张证书签过之后，"是不是同一个程序"只取决于这张证书和应用
// 标识，问一次就够了。证书怎么做见 docs/runbooks/verify-macos-launch.md。
//
//   bun run sign:mac
//
// 证书只在本机的登录钥匙串里，不进仓库，也不进持续集成：钥匙串里找不到它时说一声就
// 退出，不算失败。签的是发布目录里的应用包（连包里的浏览器桥接程序）、assemble-mcp
// 和单独构建出来的 assemble-browser-host，哪样没构建出来就跳过哪样。

import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { join } from "node:path";

/** 本地自签证书在钥匙串里的名字。运行手册里做证书的那一步用的就是它。 */
export const LOCAL_IDENTITY = "Assemble Local Signing";

type Run = (program: string, args: string[]) => string;

function run(program: string, args: string[]): string {
  try {
    return execFileSync(program, args, {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch (error) {
    const failure = error as { status?: number | null; stderr?: string };
    throw new Error(
      `${program} ${args.join(" ")} failed (${failure.status}): ${(failure.stderr ?? "").trim()}`,
    );
  }
}

/** 产物放在哪：问 cargo，另外指定过 CARGO_TARGET_DIR 的工作树也能找对。 */
export function targetDirectory(): string {
  const metadata = run("cargo", [
    "metadata",
    "--format-version",
    "1",
    "--no-deps",
  ]);
  return (JSON.parse(metadata) as { target_directory: string })
    .target_directory;
}

/**
 * 这次要签的东西，按先后排。包里的桥接程序排在包的前面：先签它，包的封印才盖在签好的
 * 文件上；反过来签，它那一份签名不在封印里，整包校验会不过。
 */
export function signTargets(
  target: string,
  exists: (path: string) => boolean = existsSync,
): string[] {
  const bundle = join(target, "release/bundle/macos/Assemble.app");
  return [
    join(bundle, "Contents/MacOS/assemble-browser-host"),
    bundle,
    join(target, "release/assemble-mcp"),
    join(target, "release/assemble-browser-host"),
  ].filter(exists);
}

/**
 * 钥匙串的签名身份清单里，叫这个名字的有几个。清单里一行长这样，自签的证书系统不信任，
 * 所以带着一句说明：
 *   2) 9756FFD1… "Assemble Local Signing" (CSSMERR_TP_NOT_TRUSTED)
 */
export function identitiesNamed(listing: string, name: string): number {
  return listing
    .split("\n")
    .filter(
      (line) =>
        /^\s*\d+\) [0-9A-F]+ "/.test(line) && line.includes(` "${name}"`),
    ).length;
}

/**
 * 给一组路径签名，再逐个严格校验。一次调用签完：系统第一次放行这把钥匙时只弹一次框。
 * codesign 签名是换一个文件进来，不是原地改，正在运行的旧进程不受影响。
 */
export function sign(
  paths: string[],
  identity: string,
  command: Run = run,
): void {
  command("/usr/bin/codesign", [
    "--force",
    "--sign",
    identity,
    "--timestamp=none",
    ...paths,
  ]);
  for (const path of paths) {
    command("/usr/bin/codesign", ["--verify", "--strict", path]);
  }
}

if (import.meta.main) {
  if (process.platform !== "darwin") {
    console.log("签名只在 macOS 上做，这里跳过。");
  } else {
    const paths = signTargets(targetDirectory());
    const listing = run("security", ["find-identity", "-p", "codesigning"]);
    const found = identitiesNamed(listing, LOCAL_IDENTITY);
    if (found > 1) {
      throw new Error(
        `钥匙串里有 ${found} 个叫"${LOCAL_IDENTITY}"的签名身份，codesign 无从选择；删掉旧的那个再试。`,
      );
    }
    if (paths.length === 0) {
      console.log("发布目录里没有构建出来的程序，跳过签名。");
    } else if (found === 0) {
      console.log(
        `钥匙串里没有"${LOCAL_IDENTITY}"，跳过签名；做法见 docs/runbooks/verify-macos-launch.md。`,
      );
    } else {
      console.log(
        `用"${LOCAL_IDENTITY}"签名。系统第一次放行这把钥匙时会弹框，点"始终允许"。`,
      );
      sign(paths, LOCAL_IDENTITY);
      for (const path of paths) console.log(`已签名：${path}`);
    }
  }
}
