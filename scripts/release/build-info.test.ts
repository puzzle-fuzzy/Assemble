import { describe, expect, test } from "bun:test";
import { readBuildInfo, resolveBuildInfo } from "./build-info.mts";

const COMMIT = "98557c34388eeefe5f8dced68249166d8133329a";

describe("构建身份", () => {
  test("正式环境的类型和官网来自同一个构建地址", () => {
    expect(
      resolveBuildInfo("0.1.0", COMMIT, {
        ASSEMBLE_SERVER_ORIGIN: "https://as.yxswy.com",
      }),
    ).toEqual({
      channel: "stable",
      version: "0.1.0",
      commit: "98557c3",
      website: "https://as.yxswy.com",
    });
  });

  test("预览地址使用实际配置，不猜测预览域名", () => {
    const info = resolveBuildInfo("0.1.0", COMMIT, {
      ASSEMBLE_SERVER_ORIGIN: "https://preview.example.test/",
    });
    expect(info.channel).toBe("preview");
    expect(info.website).toBe("https://preview.example.test");
  });

  test("默认本地地址与开发服务一致，也识别配置的内网地址", () => {
    expect(resolveBuildInfo("0.1.0", COMMIT).website).toBe(
      "http://127.0.0.1:8787",
    );
    for (const origin of [
      "http://localhost:8787",
      "https://192.168.1.20:8787",
      "https://10.0.0.8",
      "https://172.16.0.2",
    ]) {
      const info = resolveBuildInfo("0.1.0", COMMIT, {
        ASSEMBLE_SERVER_ORIGIN: origin,
      });
      expect(info.channel).toBe("local");
      expect(info.website).toBe(origin);
    }
  });

  test("三个版本类型都能明确指定，哈希统一为七位小写", () => {
    for (const [channel, origin] of [
      ["stable", "https://as.yxswy.com"],
      ["preview", "https://preview.example.test"],
      ["local", "http://127.0.0.1:8787"],
    ] as const) {
      const info = resolveBuildInfo("0.1.0", ` ${COMMIT.toUpperCase()}\n`, {
        ASSEMBLE_BUILD_CHANNEL: channel,
        ASSEMBLE_SERVER_ORIGIN: origin,
      });
      expect(info).toEqual({
        channel,
        version: "0.1.0",
        commit: "98557c3",
        website: origin,
      });
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

  test("官网不能带凭据或附带信息，版本类型必须与环境一致", () => {
    for (const origin of [
      "javascript:alert(1)",
      "https://user:secret@example.test",
      "https://example.test/path",
      "https://example.test?token=secret",
      "https://example.test#secret",
      "http://example.test",
    ]) {
      expect(() =>
        resolveBuildInfo("0.1.0", COMMIT, { ASSEMBLE_SERVER_ORIGIN: origin }),
      ).toThrow("ASSEMBLE_SERVER_ORIGIN");
    }
    expect(() =>
      resolveBuildInfo("0.1.0", COMMIT, { ASSEMBLE_BUILD_CHANNEL: "stable" }),
    ).toThrow("must match");
  });

  test("真实配置与 Git 元数据可以在 Windows 和 macOS 上读取", () => {
    const root = new URL("../../", import.meta.url);
    const info = readBuildInfo(Bun.fileURLToPath(root), {});
    expect(info.version).toBe("0.1.0");
    expect(info.channel).toBe("local");
    expect(info.commit).toMatch(/^[0-9a-f]{7}$/);
  });
});
