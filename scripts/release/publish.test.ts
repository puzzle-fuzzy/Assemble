import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  assertLocallySigned,
  type InstallerRun,
  installerRun,
} from "./publish.ts";

const SHA = "6b0d2ea27d2e4e4d0d5a3f4c8b9a0c1d2e3f4a5b";

function run(overrides: Partial<InstallerRun> = {}): InstallerRun {
  return {
    databaseId: 1,
    headBranch: "main",
    headSha: SHA,
    event: "workflow_dispatch",
    status: "completed",
    conclusion: "success",
    createdAt: "2026-11-27T02:00:00Z",
    ...overrides,
  };
}

describe("发布桌面端的更新", () => {
  test("Windows 的安装包取自这个提交跑完并且成功的那一次，几次都成功时取最新的", () => {
    const newest = run({ databaseId: 3, createdAt: "2026-11-27T03:00:00Z" });
    const others = [
      run({ databaseId: 9, headSha: "other" }),
      run({ databaseId: 8, conclusion: "failure" }),
      run({ databaseId: 7, status: "in_progress", conclusion: null }),
      run({ databaseId: 2, event: "push", headBranch: "v0.1.116" }),
      newest,
    ];
    expect(installerRun(others, SHA)?.databaseId).toBe(3);
    expect(installerRun([run({ conclusion: "failure" })], SHA)).toBeUndefined();
    expect(installerRun([], SHA)).toBeUndefined();
  });

  test.skipIf(process.platform !== "darwin")(
    "没用本地证书签的应用包发不出去，签了也不完整的不行",
    () => {
      const dir = mkdtempSync(join(tmpdir(), "assemble-publish-"));
      try {
        const bundle = join(dir, "Probe.app");
        mkdirSync(join(bundle, "Contents/MacOS"), { recursive: true });
        copyFileSync("/usr/bin/true", join(bundle, "Contents/MacOS/probe"));
        chmodSync(join(bundle, "Contents/MacOS/probe"), 0o755);
        writeFileSync(
          join(bundle, "Contents/Info.plist"),
          `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>test.assemble.publish-probe</string>
  <key>CFBundleExecutable</key><string>probe</string>
</dict></plist>`,
        );
        const unsigned = (): unknown => assertLocallySigned(bundle);
        expect(unsigned).toThrow('不是用"Assemble Local Signing"签的');
        // 临时签名同样不行：换了签名，系统就不认得更新后的应用。
        expect(
          spawnSync("/usr/bin/codesign", ["--force", "--sign", "-", bundle])
            .status,
        ).toBe(0);
        expect(unsigned).toThrow('不是用"Assemble Local Signing"签的');
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );
});
