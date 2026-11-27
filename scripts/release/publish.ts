// 发布桌面端的更新。只在发布用的那台 Mac 上跑：更新签名用的私钥只在那里，Mac 的应用包也要
// 在那里用本地证书签（换了证书，更新之后系统会再问一次钥匙串，见 scripts/release/stage.ts）。
//
//   bun run release:publish             做完前面每一步，送到服务器上只校验，不发布
//   bun run release:publish --apply     同上，校验通过就发布
//
// 它做四件事：取这个提交的 Windows 安装包（只有"安装包"流程出）；对着正式地址构建 Mac 应用
// 并用本地证书签名；给两个包签名、写发布目录（scripts/release/stage.ts）；交给
// deploy/push-update.sh。发出去的是提交，不是这台机器上没提交的改动，所以工作树必须干净。
// 完整步骤见 docs/runbooks/deploy.md。

import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { commitCount, versionFor } from "./build-desktop.ts";
import { PRODUCTION_ORIGIN } from "./build-info.mts";
import { LOCAL_IDENTITY, targetDirectory } from "./sign-mac.ts";
import { artifactNames, signingIdentity, stage } from "./stage.ts";

const root = fileURLToPath(new URL("../..", import.meta.url));

/** "安装包"流程产出的 Windows 安装包在哪个产物里（.github/workflows/release.yml）。 */
const WINDOWS_ARTIFACT = "assemble-windows-x64";

export interface InstallerRun {
  databaseId: number;
  headBranch: string;
  headSha: string;
  event: string;
  status: string;
  conclusion: string | null;
  createdAt: string;
}

/** "安装包"流程为这个提交跑完并且成功的那一次，几次都成功时取最新的一次。 */
export function installerRun(
  runs: InstallerRun[],
  sha: string,
): InstallerRun | undefined {
  return runs
    .filter(
      (run) =>
        run.headSha === sha &&
        run.status === "completed" &&
        run.conclusion === "success",
    )
    .sort((a, b) => b.createdAt.localeCompare(a.createdAt))[0];
}

/** 跑一条命令，输出原样透出来；失败了就带着退出码停下。 */
function exec(
  program: string,
  args: string[],
  environment: NodeJS.ProcessEnv = process.env,
): void {
  const result = spawnSync(program, args, {
    cwd: root,
    env: environment,
    stdio: "inherit",
  });
  if (result.status !== 0) {
    throw new Error(`${program} ${args[0]} 没有成（${result.status}）`);
  }
}

/** 跑一条命令，只要它的输出。 */
function capture(program: string, args: string[]): string {
  const result = spawnSync(program, args, {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  });
  if (result.status !== 0 || typeof result.stdout !== "string") {
    throw new Error(`${program} ${args.join(" ")} 没有成（${result.status}）`);
  }
  return result.stdout;
}

/**
 * 发出去的 Mac 应用包必须是本地那张证书签的，签名还得是完整的。
 * 签名脚本找不到证书时会跳过签名（不算失败），所以这里再认一次。
 */
export function assertLocallySigned(bundle: string): void {
  const identity = signingIdentity(bundle);
  if (identity !== LOCAL_IDENTITY) {
    throw new Error(
      `${bundle} 不是用"${LOCAL_IDENTITY}"签的（现在是${identity === null ? "临时签名或者没签" : `"${identity}"`}）。` +
        "更新之后系统就不认得它，会再问一次钥匙串。证书的做法见 docs/runbooks/verify-macos-launch.md。",
    );
  }
  const verify = spawnSync(
    "/usr/bin/codesign",
    ["--verify", "--strict", bundle],
    {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  if (verify.status !== 0) {
    throw new Error(
      `${bundle} 的签名不完整：${verify.stderr.toString().trim()}`,
    );
  }
}

function main(): void {
  const apply = process.argv.includes("--apply");
  const sha = capture("git", ["rev-parse", "HEAD"]).trim();
  if (capture("git", ["status", "--porcelain"]).trim() !== "") {
    throw new Error(
      "工作树里有没提交的改动。发出去的是提交：先提交，或者把它们收起来。",
    );
  }
  const version = versionFor(commitCount((args) => capture("git", args)));

  const runs = JSON.parse(
    capture("gh", [
      "run",
      "list",
      "--workflow",
      "release.yml",
      "--commit",
      sha,
      "--limit",
      "20",
      "--json",
      "databaseId,headBranch,headSha,event,status,conclusion,createdAt",
    ]),
  ) as InstallerRun[];
  const installer = installerRun(runs, sha);
  if (!installer) {
    throw new Error(
      `"安装包"流程还没有为 ${sha.slice(0, 7)} 成功地跑完，Windows 的安装包只能从那里来。` +
        "手动触发它（或者推一个标签），等它绿了再来。",
    );
  }

  const download = mkdtempSync(join(tmpdir(), "assemble-release-"));
  try {
    console.log(
      `版本 ${version}（${sha.slice(0, 7)}），Windows 安装包取自运行 ${installer.databaseId}`,
    );
    capture("gh", [
      "run",
      "download",
      String(installer.databaseId),
      "--name",
      WINDOWS_ARTIFACT,
      "--dir",
      download,
    ]);
    const windows = join(
      download,
      "nsis",
      artifactNames(version)["windows-x86_64"],
    );
    if (!existsSync(windows)) {
      throw new Error(
        `产物里没有 ${artifactNames(version)["windows-x86_64"]}：那次运行构建的不是版本 ${version}。`,
      );
    }

    // 对着正式地址构建：装出去的应用只认这一个服务，更新清单也在它那里。
    console.log(`构建 Mac 应用，连 ${PRODUCTION_ORIGIN}`);
    exec("bun", ["run", "build"], {
      ...process.env,
      ASSEMBLE_SERVER_ORIGIN: PRODUCTION_ORIGIN,
      // 版本由构建脚本自己算；外面要是设了这个变量，发出去的就不是这个提交的版本。
      ASSEMBLE_BUILD_VERSION: "",
    });
    const bundle = join(targetDirectory(), "release/bundle/macos/Assemble.app");
    assertLocallySigned(bundle);

    const out = join(root, "data/local/release", version);
    rmSync(out, { recursive: true, force: true });
    const staged = stage({ bundle, windows, out });
    if (staged.version !== version) {
      throw new Error(`应用包的版本是 ${staged.version}，该是 ${version}。`);
    }
    exec("deploy/push-update.sh", [
      staged.directory,
      ...(apply ? ["--apply"] : []),
    ]);
    // 发送成功就清掉；没成的那一次留着，看得出送出去的是什么。
    rmSync(out, { recursive: true, force: true });
  } finally {
    rmSync(download, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  if (process.platform !== "darwin") {
    console.error(
      "发布只在 macOS 上做：更新签名用的私钥和本地证书都在那台 Mac 上。",
    );
    process.exit(2);
  }
  try {
    main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  }
}
