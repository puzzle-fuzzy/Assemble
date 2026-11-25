import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import en from "../_locales/en/messages.json";
import zh from "../_locales/zh_CN/messages.json";
import manifest from "../manifest.json";
import { NATIVE_HOST } from "./protocol";

test("安装清单仅保留主动检测、可选网络权限和本机宿主", () => {
  expect(manifest.manifest_version).toBe(3);
  expect([...manifest.permissions].sort()).toEqual(
    [
      "activeTab",
      "scripting",
      "storage",
      "webRequest",
      "nativeMessaging",
    ].sort(),
  );
  expect([...manifest.optional_host_permissions].sort()).toEqual([
    "http://*/*",
    "https://*/*",
  ]);
  for (const forbidden of [
    "host_permissions",
    "content_scripts",
    "externally_connectable",
    "web_accessible_resources",
  ])
    expect(Object.hasOwn(manifest, forbidden)).toBe(false);
  expect(manifest.content_security_policy.extension_pages).toContain(
    "script-src 'self'; object-src 'none'; connect-src 'self'",
  );
  expect(NATIVE_HOST).toBe("com.puzzlefuzzy.assemble");
  const id = createHash("sha256")
    .update(Buffer.from(manifest.key, "base64"))
    .digest("hex")
    .slice(0, 32)
    .replace(/[0-9a-f]/g, (value) =>
      String.fromCharCode(97 + Number.parseInt(value, 16)),
    );
  expect(id).toBe("mbdcafmapadhggadghdibebbgfpldhge");
  expect(manifest.default_locale).toBe("zh_CN");
  expect(Object.keys(en).sort()).toEqual(Object.keys(zh).sort());
  for (const value of [
    manifest.name,
    manifest.description,
    manifest.action.default_title,
  ]) {
    const key = /^__MSG_(\w+)__$/.exec(value)?.[1];
    expect(key).toBeDefined();
    expect(zh[key as keyof typeof zh]?.message).toBeTruthy();
    expect(en[key as keyof typeof en]?.message).toBeTruthy();
  }
});
