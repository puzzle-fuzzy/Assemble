// 把构建出来的桌面应用做成交给服务端的发布目录：Mac 的应用包打成 tar.gz，连同 Windows 的
// 安装包各签一次名，再写一份清单。发布本身在 scripts/release/publish.ts。
//
//   bun scripts/release/stage.ts --bundle <Assemble.app> [--windows <安装包>] --out <目录>
//
// 发布目录里只有这些，有别的文件服务端会拒收：
//   manifest.json
//   Assemble_<版本>_aarch64.app.tar.gz 和它的 .sig
//   Assemble_<版本>_x64-setup.exe 和它的 .sig（给了 Windows 的安装包才有）
//
// 签名用 ~/.assemble/updater.key：只在这台 Mac 上，没有口令。这里只把路径交给 tauri signer sign，
// 不读它的内容，也不把它写进任何输出。签名时带上版本：服务端和桌面应用都要求签名是为清单上的
// 那一个版本签的。应用里编着公钥，签出来对不上的包两边都不会装。
//
// tar.gz 的样子由 Tauri 的更新插件定：顶层就是 Assemble.app，安装时插件把它的内容整个换进
// 正在运行的那个应用包。只打应用包本身，不带 macOS 的元数据文件。

import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));

/** 更新签名用的私钥。只在发布用的那台 Mac 上。 */
export const UPDATER_KEY = join(homedir(), ".assemble", "updater.key");

type Platform = "darwin-aarch64" | "windows-x86_64";

function run(
  program: string,
  args: string[],
  cwd: string,
  environment: NodeJS.ProcessEnv = process.env,
): void {
  const result = spawnSync(program, args, {
    cwd,
    env: environment,
    stdio: ["ignore", "inherit", "inherit"],
  });
  if (result.status !== 0) {
    throw new Error(`${program} ${args[0]} failed (${result.status})`);
  }
}

/** 应用包自己说的版本和标识。清单上的版本从这里来，免得把不是这一版的包发出去。 */
export function bundleInfo(bundle: string): {
  version: string;
  identifier: string;
} {
  const read = (key: string): string => {
    const result = spawnSync(
      "/usr/bin/plutil",
      ["-extract", key, "raw", "-o", "-", join(bundle, "Contents/Info.plist")],
      { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
    );
    if (result.status !== 0) throw new Error(`${bundle} 里读不到 ${key}`);
    return result.stdout.trim();
  };
  const version = read("CFBundleShortVersionString");
  if (!/^\d+\.\d+\.\d+$/.test(version)) {
    throw new Error(
      `${bundle} 的版本是"${version}"，不是 0.1.116 这样的三段数字`,
    );
  }
  return { version, identifier: read("CFBundleIdentifier") };
}

/** 两个平台的更新包各叫什么。 */
export function artifactNames(version: string): Record<Platform, string> {
  return {
    "darwin-aarch64": `Assemble_${version}_aarch64.app.tar.gz`,
    "windows-x86_64": `Assemble_${version}_x64-setup.exe`,
  };
}

/**
 * `codesign -dv` 的输出里，应用包是用哪张证书签的。临时签名（adhoc）和没签都是 null：
 * 为什么要在乎这个，见 signingIdentity。
 */
export function identityFromListing(listing: string): string | null {
  return /^Authority=(.+)$/m.exec(listing)?.[1] ?? null;
}

/**
 * 应用包是用哪张证书签的。
 *
 * 更新之后的应用必须是同一张证书签的：系统认"是不是同一个程序"只看证书和应用标识，换了证书，
 * 读钥匙串里存的登录状态时就又要问一次。所以发出去的包只能是本地那张证书签的，临时签名的不行。
 */
export function signingIdentity(bundle: string): string | null {
  // 说明文字走标准错误，两边合起来看。
  const result = spawnSync(
    "/usr/bin/codesign",
    ["-dv", "--verbose=2", bundle],
    {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  return result.status === 0
    ? identityFromListing(`${result.stdout}${result.stderr}`)
    : null;
}

/** 发布目录里的清单。服务端按这个格式读，字段名不能改。 */
export function stagingManifest(input: {
  version: string;
  notes: string;
  publishedAt: Date;
  platforms: { platform: Platform; file: string }[];
}): string {
  return `${JSON.stringify(
    {
      version: input.version,
      notes: input.notes,
      pubDate: input.publishedAt.toISOString(),
      platforms: input.platforms,
    },
    null,
    2,
  )}\n`;
}

/** 给更新包签名：旁边多出一个 .sig。签名是公开的，这里不打印它，只报成功与否。 */
export function signArtifact(
  file: string,
  version: string,
  keyPath: string = UPDATER_KEY,
): void {
  if (!existsSync(keyPath)) {
    throw new Error(
      `找不到更新签名用的私钥 ${keyPath}。它只在发布用的那台 Mac 上，做法见 docs/runbooks/deploy.md。`,
    );
  }
  // 从桌面应用的目录里调：tauri 的命令行工具是它的开发依赖。钥匙没有口令，标准输入关掉，
  // 它不会停下来问。
  const result = spawnSync(
    "bun",
    [
      "x",
      "tauri",
      "signer",
      "sign",
      "--private-key-path",
      keyPath,
      "--app-version",
      version,
      file,
    ],
    {
      cwd: join(root, "apps/desktop"),
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  if (result.status !== 0 || !existsSync(`${file}.sig`)) {
    throw new Error(
      `${basename(file)} 没有签成：${result.stderr.toString().trim()}`,
    );
  }
}

/**
 * 做出发布目录。目录可以不存在，也可以是空的；里面有东西就不做，免得旧文件混进去。
 * Windows 的安装包可以不给：只发 Mac 的那一版（本机端到端测试用），或者那个平台还没发布过。
 */
export function stage(options: {
  bundle: string;
  windows?: string;
  out: string;
  keyPath?: string;
  notes?: string;
}): { version: string; directory: string } {
  const { version, identifier } = bundleInfo(options.bundle);
  const names = artifactNames(version);
  if (
    options.windows &&
    basename(options.windows) !== names["windows-x86_64"]
  ) {
    throw new Error(
      `Windows 的安装包该叫 ${names["windows-x86_64"]}，拿来的是 ${basename(options.windows)}：两边不是同一个版本。`,
    );
  }
  mkdirSync(options.out, { recursive: true });
  if (readdirSync(options.out).length > 0) {
    throw new Error(
      `${options.out} 不是空的；发布目录里只能有这一次放进去的东西。`,
    );
  }

  const mac = join(options.out, names["darwin-aarch64"]);
  // COPYFILE_DISABLE：不往包里塞 macOS 的元数据文件，别的系统解出来会是多出来的东西。
  run(
    "tar",
    ["czf", mac, "-C", dirname(options.bundle), basename(options.bundle)],
    root,
    { ...process.env, COPYFILE_DISABLE: "1" },
  );

  const platforms: { platform: Platform; file: string }[] = [
    { platform: "darwin-aarch64", file: names["darwin-aarch64"] },
  ];
  if (options.windows) {
    copyFileSync(options.windows, join(options.out, names["windows-x86_64"]));
    platforms.push({
      platform: "windows-x86_64",
      file: names["windows-x86_64"],
    });
  }
  for (const { file } of platforms) {
    signArtifact(join(options.out, file), version, options.keyPath);
  }
  writeFileSync(
    join(options.out, "manifest.json"),
    stagingManifest({
      version,
      notes: options.notes ?? `Assemble ${version}`,
      publishedAt: new Date(),
      platforms,
    }),
  );
  console.log(`应用包 ${identifier}，版本 ${version}；发布目录 ${options.out}`);
  return { version, directory: options.out };
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const value = (flag: string): string | undefined => {
    const index = args.indexOf(flag);
    return index === -1 ? undefined : args[index + 1];
  };
  const bundle = value("--bundle");
  const out = value("--out");
  if (!bundle || !out || !existsSync(bundle)) {
    console.error(
      "用法：bun scripts/release/stage.ts --bundle <Assemble.app> [--windows <安装包>] --out <目录>",
    );
    process.exit(2);
  }
  stage({ bundle, windows: value("--windows"), out });
}
