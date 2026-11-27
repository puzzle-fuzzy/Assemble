import { describe, expect, test } from "bun:test";
import {
  buildArguments,
  buildVersion,
  commitCount,
  versionFor,
} from "./build-desktop.ts";

describe("打包桌面应用", () => {
  test("版本是 0.1 加提交数", () => {
    expect(versionFor(1)).toBe("0.1.1");
    expect(versionFor(431)).toBe("0.1.431");
    expect(
      commitCount((args) => {
        expect(args).toEqual(["rev-list", "--count", "HEAD"]);
        return "431\n";
      }),
    ).toBe(431);
  });

  test("提交数不是正整数就失败：浅克隆算出来的提交数不能用", () => {
    for (const output of ["", "0", "-3", "1.5", "many"]) {
      expect(() => commitCount(() => output)).toThrow("commit count");
    }
  });

  test("版本缺省是算出来的那一个，明确指定时才用指定的", () => {
    expect(buildVersion({}, 431)).toBe("0.1.431");
    expect(buildVersion({ ASSEMBLE_BUILD_VERSION: "" }, 431)).toBe("0.1.431");
    expect(buildVersion({ ASSEMBLE_BUILD_VERSION: " 0.1.1000 " }, 431)).toBe(
      "0.1.1000",
    );
    // 指定的版本会装进应用并被自动更新拿来比先后：只认三段数字。
    for (const version of ["0.1", "latest", "0.1.431-beta", "v0.1.431"]) {
      expect(() =>
        buildVersion({ ASSEMBLE_BUILD_VERSION: version }, 431),
      ).toThrow("ASSEMBLE_BUILD_VERSION");
    }
  });

  test("版本经 --config 并进配置，其余参数原样交给 tauri build", () => {
    expect(buildArguments("0.1.431", [])).toEqual([
      "x",
      "tauri",
      "build",
      "--config",
      '{"version":"0.1.431"}',
    ]);
    // 另一份 --config 排在后面：Tauri 按顺序合并，后一份盖住前一份的同名项。
    expect(
      buildArguments("0.1.1000", [
        "--debug",
        "--config",
        '{"identifier":"test.assemble.updater"}',
      ]),
    ).toEqual([
      "x",
      "tauri",
      "build",
      "--config",
      '{"version":"0.1.1000"}',
      "--debug",
      "--config",
      '{"identifier":"test.assemble.updater"}',
    ]);
  });
});
