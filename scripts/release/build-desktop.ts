// 打包桌面应用：算出版本号，交给 tauri build；在 macOS 上最后用本地证书签一次名。
//
// 版本是 0.1.<提交数>，它也是自动更新认先后的依据。同一个提交在 Mac 和 Windows 上是同一个号，
// 所以一律从这里跑（`bun run build`，持续集成也是），不要直接调 tauri build：那样版本停在配置里
// 的那一个，装出去的应用会把它当成最旧的版本。为什么这样做见 docs/decisions/0015-desktop-auto-update.md。
//
//   bun run build                      发布构建，连哪个服务由 ASSEMBLE_SERVER_ORIGIN 决定
//   bun run build -- --debug --bundles app --config '{"identifier":"…"}'
//                                      破折号后面的原样交给 tauri build
//   bun scripts/release/build-desktop.ts --version
//                                      只打印这个提交会用的版本号
//
// 签名（`bun run sign:mac`）排在构建之后：只签发布目录里的应用包，调试构建那里没有东西可签。
// 自动更新用的签名不在这里：那是发布时的事，见 scripts/release/stage.ts。

import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));

type Environment = Record<string, string | undefined>;

/** 这个提交是仓库里的第几个提交：在目标上跑 `git rev-list --count HEAD`。 */
export function commitCount(run: (args: string[]) => string): number {
  const count = Number(run(["rev-list", "--count", "HEAD"]).trim());
  if (!Number.isSafeInteger(count) || count < 1) {
    throw new Error(
      "The commit count must be a positive integer; build from a full Git checkout (fetch-depth: 0)",
    );
  }
  return count;
}

/** 自动更新认的版本号：每次提交加一。 */
export function versionFor(count: number): string {
  return `0.1.${count}`;
}

/**
 * 这次构建用的版本。缺省是算出来的那一个；`ASSEMBLE_BUILD_VERSION` 可以明确指定另一个，
 * 本机的端到端测试要构建两个相邻的版本时用。指定了就不看提交数，所以发布前不要设置它
 * （发布的脚本自己会核对标签与版本，见 scripts/release/publish.ts）。
 */
export function buildVersion(environment: Environment, count: number): string {
  const override = environment.ASSEMBLE_BUILD_VERSION?.trim();
  if (override === undefined || override === "") return versionFor(count);
  if (!/^\d+\.\d+\.\d+$/.test(override)) {
    throw new Error(
      "ASSEMBLE_BUILD_VERSION must be a version like 0.1.431, found something else",
    );
  }
  return override;
}

/** 交给 tauri build 的参数：版本经 `--config` 并进配置，其余原样转交。 */
export function buildArguments(
  version: string,
  forwarded: readonly string[],
): string[] {
  return [
    "x",
    "tauri",
    "build",
    "--config",
    JSON.stringify({ version }),
    ...forwarded,
  ];
}

function runGit(args: string[]): string {
  const result = spawnSync("git", args, {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  });
  if (result.status !== 0 || typeof result.stdout !== "string") {
    throw new Error(`git ${args.join(" ")} failed (${result.status})`);
  }
  return result.stdout;
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const version = buildVersion(process.env, commitCount(runGit));
  if (args[0] === "--version") {
    console.log(version);
    process.exit(0);
  }
  const forwarded = args[0] === "--" ? args.slice(1) : args;
  // 界面构建（vite）读的是这个环境变量：配置文件里的版本不是这次真正装进应用的那一个。
  const environment = { ...process.env, ASSEMBLE_BUILD_VERSION: version };
  const build = spawnSync("bun", buildArguments(version, forwarded), {
    cwd: join(root, "apps/desktop"),
    stdio: "inherit",
    env: environment,
  });
  if (build.status !== 0) process.exit(build.status ?? 1);
  // 签名脚本只认发布目录里的应用包；调试构建在别处，没有什么可签，不去动上一次的发布构建。
  if (forwarded.includes("--debug")) process.exit(0);
  const sign = spawnSync("bun", ["run", "sign:mac"], {
    cwd: root,
    stdio: "inherit",
    env: environment,
  });
  process.exit(sign.status ?? 1);
}
