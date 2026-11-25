// 构建时只向界面注入公开的构建身份和官网来源，避免把环境变量带进安装包。
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { BuildInfo } from "../../apps/desktop/src/lib/build-identity.ts";

type Environment = Record<string, string | undefined>;

const LOCAL_ORIGIN = "http://127.0.0.1:8787";
const PRODUCTION_ORIGIN = "https://as.yxswy.com";

function websiteOrigin(value: string): URL {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error("ASSEMBLE_SERVER_ORIGIN must be a valid origin");
  }
  const loopback = ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname);
  if (
    (url.protocol !== "https:" && !(url.protocol === "http:" && loopback)) ||
    url.username ||
    url.password ||
    url.pathname !== "/" ||
    url.search ||
    url.hash
  ) {
    throw new Error(
      "ASSEMBLE_SERVER_ORIGIN must be an HTTPS origin or HTTP loopback origin without credentials",
    );
  }
  return url;
}

function isLocalWebsite(url: URL): boolean {
  if (["localhost", "[::1]"].includes(url.hostname)) return true;
  const parts = url.hostname.split(".").map(Number);
  if (parts.length !== 4 || !parts.every(Number.isInteger)) return false;
  return (
    parts[0] === 127 ||
    parts[0] === 10 ||
    (parts[0] === 192 && parts[1] === 168) ||
    (parts[0] === 172 &&
      parts[1] !== undefined &&
      parts[1] >= 16 &&
      parts[1] <= 31)
  );
}

export function resolveBuildInfo(
  version: string,
  commit: string,
  environment: Environment = {},
): BuildInfo {
  const website = websiteOrigin(
    environment.ASSEMBLE_SERVER_ORIGIN?.trim() || LOCAL_ORIGIN,
  );
  const configured = environment.ASSEMBLE_BUILD_CHANNEL?.trim();
  const detectedChannel = isLocalWebsite(website)
    ? "local"
    : website.origin === PRODUCTION_ORIGIN
      ? "stable"
      : "preview";
  const channel = configured || detectedChannel;
  if (channel !== "stable" && channel !== "preview" && channel !== "local") {
    throw new Error("ASSEMBLE_BUILD_CHANNEL must be stable, preview, or local");
  }
  if (channel !== detectedChannel) {
    throw new Error(
      "ASSEMBLE_BUILD_CHANNEL must match the website environment",
    );
  }
  if (!/^[0-9a-f]{7,64}$/i.test(commit.trim())) {
    throw new Error("The build commit must be a Git commit hash");
  }
  return {
    channel,
    version,
    commit: commit.trim().slice(0, 7).toLowerCase(),
    website: website.origin,
  };
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
