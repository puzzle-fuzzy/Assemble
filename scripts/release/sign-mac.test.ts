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
  identitiesNamed,
  LOCAL_IDENTITY,
  sign,
  signTargets,
} from "./sign-mac.ts";

// 一份像模像样的清单：别人的证书、我们那张（自签的，系统不信任）、结尾的统计。
const LISTING = `
Policy: Code Signing
  Matching identities
  1) 264AA190BEC38AC4E29133D5DEED56A6A6D3C854 "Apple Development: someone@example.com (CB66NPS4UB)"
  2) 9756FFD1D4E31C3F3A1B2C9D0E5F7A8B9C0D1E "${LOCAL_IDENTITY}" (CSSMERR_TP_NOT_TRUSTED)
     2 identities found
`;

describe("本地签名", () => {
  test("包里的桥接程序排在包的前面，没构建出来的跳过", () => {
    const everything = signTargets("/target", () => true);
    expect(everything).toEqual([
      "/target/release/bundle/macos/Assemble.app/Contents/MacOS/assemble-browser-host",
      "/target/release/bundle/macos/Assemble.app",
      "/target/release/assemble-mcp",
      "/target/release/assemble-browser-host",
    ]);
    expect(
      signTargets("/target", (path) => path.endsWith("assemble-mcp")),
    ).toEqual(["/target/release/assemble-mcp"]);
    expect(signTargets("/target", () => false)).toEqual([]);
  });

  test("在钥匙串的身份清单里按名字数，不把名字相近的算进去", () => {
    expect(identitiesNamed(LISTING, LOCAL_IDENTITY)).toBe(1);
    expect(identitiesNamed(LISTING, "Assemble Local")).toBe(0);
    expect(identitiesNamed(LISTING, "Apple Development")).toBe(0);
    expect(identitiesNamed("", LOCAL_IDENTITY)).toBe(0);
    expect(identitiesNamed(`${LISTING}${LISTING}`, LOCAL_IDENTITY)).toBe(2);
  });

  test("一次调用签完所有路径，再逐个严格校验；签名失败就不校验", () => {
    const calls: string[][] = [];
    const record = (program: string, args: string[]) => {
      calls.push([program, ...args]);
      return "";
    };
    sign(["/a", "/b"], "Assemble Local Signing", record);
    expect(calls).toEqual([
      [
        "/usr/bin/codesign",
        "--force",
        "--sign",
        "Assemble Local Signing",
        "--timestamp=none",
        "/a",
        "/b",
      ],
      ["/usr/bin/codesign", "--verify", "--strict", "/a"],
      ["/usr/bin/codesign", "--verify", "--strict", "/b"],
    ]);

    calls.length = 0;
    expect(() =>
      sign(["/a"], "Assemble Local Signing", () => {
        throw new Error("the user declined");
      }),
    ).toThrow("the user declined");
    expect(calls).toEqual([]);
  });

  // 用临时的身份真的签一遍：系统自带的任何程序都能签，不需要钥匙串里有什么。
  test.skipIf(process.platform !== "darwin")(
    "先签包里的程序再签包，整包校验能过，签好的那一份在包的封印里",
    () => {
      const dir = mkdtempSync(join(tmpdir(), "assemble-sign-"));
      try {
        const macos = join(dir, "Probe.app/Contents/MacOS");
        mkdirSync(macos, { recursive: true });
        writeFileSync(
          join(dir, "Probe.app/Contents/Info.plist"),
          `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>test.assemble.signing-probe</string>
  <key>CFBundleExecutable</key><string>probe</string>
</dict></plist>`,
        );
        for (const name of ["probe", "assemble-browser-host"]) {
          copyFileSync("/usr/bin/true", join(macos, name));
          chmodSync(join(macos, name), 0o755);
        }
        const helper = join(macos, "assemble-browser-host");
        const bundle = join(dir, "Probe.app");
        sign([helper, bundle], "-");

        // codesign 的说明文字走标准错误，两边一起看。
        const probe = (args: string[]) => {
          const result = spawnSync("/usr/bin/codesign", args, {
            encoding: "utf8",
          });
          return { status: result.status, text: result.stdout + result.stderr };
        };
        expect(probe(["-dv", bundle]).text).toContain(
          "Identifier=test.assemble.signing-probe",
        );
        expect(probe(["-dv", helper]).text).toContain("Signature=adhoc");
        // 包签好之后再动包里的文件，封印就对不上：说明签好的桥接程序确实被封在里面。
        writeFileSync(helper, "changed after signing", { flag: "a" });
        expect(probe(["--verify", "--strict", bundle]).status).not.toBe(0);
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    },
  );
});
