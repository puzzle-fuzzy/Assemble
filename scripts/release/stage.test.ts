import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  artifactNames,
  bundleInfo,
  identityFromListing,
  signArtifact,
  stage,
  stagingManifest,
  UPDATER_KEY,
} from "./stage.ts";

// 真的自签证书在钥匙串里的那一行；别的两行是另外两种情况。
const LOCAL = `
Executable=/Applications/Assemble.app/Contents/MacOS/assemble-desktop
Identifier=com.puzzlefuzzy.assemble
Format=app bundle with Mach-O thin (arm64)
Signature size=1782
Authority=Assemble Local Signing
Info.plist entries=33
TeamIdentifier=not set
Sealed Resources version=2 rules=13 files=0
`;
const ADHOC = `
Executable=/tmp/Assemble.app/Contents/MacOS/assemble-desktop
Identifier=com.puzzlefuzzy.assemble
Signature=adhoc
Info.plist entries=33
TeamIdentifier=not set
`;

/** 一个像应用包的目录：能跑的程序、说得出版本和标识的 Info.plist。 */
function fakeBundle(dir: string, version: string): string {
  const bundle = join(dir, "Assemble.app");
  mkdirSync(join(bundle, "Contents/MacOS"), { recursive: true });
  copyFileSync(
    "/usr/bin/true",
    join(bundle, "Contents/MacOS/assemble-desktop"),
  );
  chmodSync(join(bundle, "Contents/MacOS/assemble-desktop"), 0o755);
  writeFileSync(
    join(bundle, "Contents/Info.plist"),
    `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>test.assemble.stage-probe</string>
  <key>CFBundleExecutable</key><string>assemble-desktop</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
</dict></plist>`,
  );
  return bundle;
}

describe("做出发布目录", () => {
  test("两个平台的更新包的名字里带着版本", () => {
    expect(artifactNames("0.1.116")).toEqual({
      "darwin-aarch64": "Assemble_0.1.116_aarch64.app.tar.gz",
      "windows-x86_64": "Assemble_0.1.116_x64-setup.exe",
    });
  });

  test("清单的字段名和服务端读的一致，时间是 RFC 3339", () => {
    const text = stagingManifest({
      version: "0.1.116",
      notes: "修了什么",
      publishedAt: new Date("2026-11-27T01:30:00.000Z"),
      platforms: [
        { platform: "windows-x86_64", file: "Assemble_0.1.116_x64-setup.exe" },
      ],
    });
    expect(JSON.parse(text)).toEqual({
      version: "0.1.116",
      notes: "修了什么",
      pubDate: "2026-11-27T01:30:00.000Z",
      platforms: [
        { platform: "windows-x86_64", file: "Assemble_0.1.116_x64-setup.exe" },
      ],
    });
    expect(text.endsWith("}\n")).toBe(true);
  });

  test("从 codesign 的输出里认出签名用的证书，临时签名和没签都不算", () => {
    expect(identityFromListing(LOCAL)).toBe("Assemble Local Signing");
    expect(identityFromListing(ADHOC)).toBeNull();
    expect(identityFromListing("")).toBeNull();
  });

  test.skipIf(process.platform !== "darwin")(
    "应用包的版本从 Info.plist 里读；认不出的写法不拿来当版本",
    () => {
      const dir = mkdtempSync(join(tmpdir(), "assemble-stage-"));
      try {
        expect(bundleInfo(fakeBundle(dir, "0.1.116"))).toEqual({
          version: "0.1.116",
          identifier: "test.assemble.stage-probe",
        });
        expect(() => bundleInfo(fakeBundle(dir, "0.1"))).toThrow("三段数字");
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  test("私钥不在这台机器上时说清楚，不留下半个发布目录", () => {
    expect(() =>
      signArtifact("/nonexistent/file", "0.1.116", "/nonexistent/updater.key"),
    ).toThrow("updater.key");
  });

  // 做发布目录这件事只在 macOS 上有意义（要读 Info.plist、打 tar 包），所以这两条也只在 macOS 上跑。
  test.skipIf(process.platform !== "darwin")(
    "Windows 的安装包和 Mac 应用包不是同一个版本就不做",
    () => {
      const dir = mkdtempSync(join(tmpdir(), "assemble-stage-"));
      try {
        mkdirSync(join(dir, "installer"));
        writeFileSync(
          join(dir, "installer/Assemble_0.1.115_x64-setup.exe"),
          "x",
        );
        const out = join(dir, "out");
        expect(() =>
          stage({
            bundle: fakeBundle(dir, "0.1.116"),
            windows: join(dir, "installer/Assemble_0.1.115_x64-setup.exe"),
            out,
          }),
        ).toThrow("不是同一个版本");
        // 认出版本不对就停，还没有动发布目录。
        expect(existsSync(out)).toBe(false);
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  test.skipIf(process.platform !== "darwin")(
    "发布目录里有东西就不往里放",
    () => {
      const dir = mkdtempSync(join(tmpdir(), "assemble-stage-"));
      try {
        const out = join(dir, "out");
        mkdirSync(out);
        writeFileSync(join(out, "manifest.json"), "{}");
        // 安装包的名字和版本都对得上，所以这一回拦在目录检查上。
        mkdirSync(join(dir, "installer"));
        writeFileSync(
          join(dir, "installer/Assemble_0.1.116_x64-setup.exe"),
          "x",
        );
        expect(() =>
          stage({
            bundle: fakeBundle(dir, "0.1.116"),
            windows: join(dir, "installer/Assemble_0.1.116_x64-setup.exe"),
            out,
          }),
        ).toThrow("不是空的");
        expect(readdirSync(out)).toEqual(["manifest.json"]);
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );

  // 真的用发布用的那把钥匙签一次：只有那台 Mac 上有，持续集成上跳过。签的是一个临时的
  // 假应用包，签完就删，不发布任何东西。
  test.skipIf(process.platform !== "darwin" || !existsSync(UPDATER_KEY))(
    "打出来的 tar.gz 顶层就是应用包，签名是为这个版本签的，目录里没有别的文件",
    () => {
      const dir = mkdtempSync(join(tmpdir(), "assemble-stage-"));
      try {
        const bundle = fakeBundle(dir, "0.1.116");
        const out = join(dir, "out");
        const staged = stage({ bundle, out });
        expect(staged.version).toBe("0.1.116");
        const names = artifactNames("0.1.116");
        expect(readdirSync(out).sort()).toEqual([
          names["darwin-aarch64"],
          `${names["darwin-aarch64"]}.sig`,
          "manifest.json",
        ]);
        // 更新插件把压缩包顶层那一段去掉，剩下的整个换进应用包：顶层必须只有应用包自己。
        const listing = spawnSync(
          "tar",
          ["tzf", join(out, names["darwin-aarch64"])],
          { encoding: "utf8" },
        );
        const entries = listing.stdout.trim().split("\n");
        expect(entries[0]).toBe("Assemble.app/");
        expect(
          entries.every((entry) => entry.startsWith("Assemble.app/")),
        ).toBe(true);
        expect(entries.some((entry) => entry.includes("/._"))).toBe(false);

        const signature = Buffer.from(
          readFileSync(
            join(out, `${names["darwin-aarch64"]}.sig`),
            "utf8",
          ).trim(),
          "base64",
        ).toString("utf8");
        expect(signature).toContain("version:0.1.116");
        expect(signature).toContain("file:Assemble_0.1.116_aarch64.app.tar.gz");

        const manifest = JSON.parse(
          readFileSync(join(out, "manifest.json"), "utf8"),
        );
        expect(manifest.version).toBe("0.1.116");
        expect(manifest.platforms).toEqual([
          { platform: "darwin-aarch64", file: names["darwin-aarch64"] },
        ]);
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );
});
