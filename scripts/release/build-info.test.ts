import { describe, expect, test } from "bun:test";
import { formatBuildInfo } from "../../apps/desktop/src/lib/build-info";
import { readBuildInfo, resolveBuildInfo } from "./build-info.mts";

const COMMIT = "98557c34388eeefe5f8dced68249166d8133329a";

describe("构建身份", () => {
  test("本机默认是本地测试，连接正式服务也不改变版本类型", () => {
    expect(
      resolveBuildInfo("0.1.0", COMMIT, {
        ASSEMBLE_SERVER_ORIGIN: "https://as.yxswy.com",
      }),
    ).toEqual({ channel: "local", version: "0.1.0", commit: "98557c3" });
  });

  test("与应用版本一致的稳定标签才自动认作正式版本", () => {
    const environment = {
      GITHUB_ACTIONS: "true",
      GITHUB_REF_TYPE: "tag",
      GITHUB_REF_NAME: "v0.1.0",
    };
    expect(resolveBuildInfo("0.1.0", COMMIT, environment).channel).toBe(
      "stable",
    );
    expect(resolveBuildInfo("0.2.0", COMMIT, environment).channel).toBe(
      "preview",
    );
    expect(
      resolveBuildInfo("0.1.0-rc.1", COMMIT, {
        ...environment,
        GITHUB_REF_NAME: "v0.1.0-rc.1",
      }).channel,
    ).toBe("preview");
  });

  test("持续集成里的分支和手动构建默认是预览版本", () => {
    expect(
      resolveBuildInfo("0.1.0", COMMIT, {
        GITHUB_ACTIONS: "true",
        GITHUB_REF_TYPE: "branch",
        GITHUB_REF_NAME: "main",
      }).channel,
    ).toBe("preview");
  });

  test("三个版本类型都能明确指定，格式固定且哈希统一为七位小写", () => {
    for (const [channel, name] of [
      ["stable", "正式版本"],
      ["preview", "预览版本"],
      ["local", "本地测试"],
    ]) {
      const info = resolveBuildInfo("0.1.0", ` ${COMMIT.toUpperCase()}\n`, {
        ASSEMBLE_BUILD_CHANNEL: channel,
        GITHUB_ACTIONS: "true",
      });
      expect(formatBuildInfo(info)).toBe(`${name} - 0.1.0.98557c3`);
    }
  });

  test("非法类型或不真实的哈希使构建失败，不用占位值冒充身份", () => {
    expect(() =>
      resolveBuildInfo("0.1.0", COMMIT, { ASSEMBLE_BUILD_CHANNEL: "release" }),
    ).toThrow("ASSEMBLE_BUILD_CHANNEL");
    expect(() => resolveBuildInfo("0.1.0", "unknown")).toThrow(
      "Git commit hash",
    );
  });

  test("真实配置与 Git 元数据可以在 Windows 和 macOS 上读取", () => {
    const root = new URL("../../", import.meta.url);
    const info = readBuildInfo(Bun.fileURLToPath(root), {});
    expect(info.version).toBe("0.1.0");
    expect(info.channel).toBe("local");
    expect(info.commit).toMatch(/^[0-9a-f]{7}$/);
  });
});
