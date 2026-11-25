// 构建时只向界面注入这三个公开字段，避免把构建机的环境变量带进安装包。
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { BuildInfo } from "../../apps/desktop/src/lib/build-info.ts";

type Environment = Record<string, string | undefined>;

export function resolveBuildInfo(
  version: string,
  commit: string,
  environment: Environment = {},
): BuildInfo {
  const configured = environment.ASSEMBLE_BUILD_CHANNEL?.trim();
  const channel =
    configured ||
    (environment.GITHUB_ACTIONS === "true"
      ? environment.GITHUB_REF_TYPE === "tag" &&
        environment.GITHUB_REF_NAME === `v${version}` &&
        /^\d+\.\d+\.\d+$/.test(version)
        ? "stable"
        : "preview"
      : "local");
  if (channel !== "stable" && channel !== "preview" && channel !== "local") {
    throw new Error("ASSEMBLE_BUILD_CHANNEL must be stable, preview, or local");
  }
  if (!/^[0-9a-f]{7,64}$/i.test(commit.trim())) {
    throw new Error("The build commit must be a Git commit hash");
  }
  return { channel, version, commit: commit.trim().slice(0, 7).toLowerCase() };
}

export function readBuildInfo(
  root: string,
  environment: Environment = process.env,
): BuildInfo {
  const config = JSON.parse(
    readFileSync(join(root, "apps/desktop/src-tauri/tauri.conf.json"), "utf8"),
  ) as { version?: unknown };
  if (typeof config.version !== "string" || config.version.length === 0) {
    throw new Error("tauri.conf.json must define the application version");
  }
  let commit = environment.ASSEMBLE_BUILD_COMMIT?.trim();
  if (!commit) {
    try {
      // 用参数数组调用 Git，Windows 和 macOS 共用同一条路径，不经 shell 拼接。
      commit = execFileSync("git", ["rev-parse", "HEAD"], {
        cwd: root,
        encoding: "utf8",
        windowsHide: true,
        stdio: ["ignore", "pipe", "ignore"],
      }).trim();
    } catch {
      throw new Error(
        "The build commit is unavailable; build from Git or set ASSEMBLE_BUILD_COMMIT",
      );
    }
  }
  return resolveBuildInfo(config.version, commit, environment);
}
