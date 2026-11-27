import { afterEach, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import invalidTimes from "../../apps/releases-web/contract/invalid-times.json";
import {
  checkReleases,
  type PublishOptions,
  publishArtifact,
} from "./publish-artifact";

const testingRoot = resolve(import.meta.dir, "../../data/local/releases-tests");
const owned: string[] = [];
afterEach(async () => {
  for (const directory of owned.splice(0)) {
    if (
      !directory.startsWith(`${testingRoot}\\`) &&
      !directory.startsWith(`${testingRoot}/`)
    )
      throw new Error("Unsafe test cleanup");
    await rm(directory, { recursive: true, force: true });
  }
});
async function setup() {
  await mkdir(testingRoot, { recursive: true });
  const directory = await mkdtemp(join(testingRoot, "case-"));
  owned.push(directory);
  const source = join(directory, "installer.exe");
  await writeFile(source, "MZ first package");
  const options: PublishOptions = {
    directory: join(directory, "releases"),
    source,
    platform: "windows",
    architecture: "x64",
    version: "0.1.0.abc1234",
  };
  return { directory, source, options };
}
test("发布实际字节、校验摘要并原子写清单，重复执行不改身份，旧下载仍保留", async () => {
  const { source, options } = await setup();
  const artifact = await publishArtifact(options);
  expect(artifact.sha256).toBe(
    createHash("sha256").update("MZ first package").digest("hex"),
  );
  expect(
    await readFile(join(options.directory, "objects", artifact.sha256), "utf8"),
  ).toBe("MZ first package");
  const before = await readFile(
    join(options.directory, "manifest.json"),
    "utf8",
  );
  expect(await publishArtifact(options)).toEqual(artifact);
  expect(await readFile(join(options.directory, "manifest.json"), "utf8")).toBe(
    before,
  );
  await writeFile(source, "MZ second package");
  await publishArtifact({ ...options, version: "0.2.0.def1234" });
  const checked = await checkReleases(options.directory);
  expect(checked.artifacts).toHaveLength(2);
  expect(
    await readFile(join(options.directory, "objects", artifact.sha256), "utf8"),
  ).toBe("MZ first package");
});
test("相同版本不同字节拒绝覆盖，现有清单与安装包完整保留", async () => {
  const { source, options } = await setup();
  await publishArtifact(options);
  const before = await readFile(
    join(options.directory, "manifest.json"),
    "utf8",
  );
  await writeFile(source, "MZ different package");
  await expect(publishArtifact(options)).rejects.toThrow(
    "RELEASE_IDENTITY_CONFLICT",
  );
  expect(await readFile(join(options.directory, "manifest.json"), "utf8")).toBe(
    before,
  );
  expect((await checkReleases(options.directory)).artifacts).toHaveLength(1);
});
test("损坏对象不会被当成幂等成功，也不会被覆盖修复", async () => {
  const { options } = await setup();
  const artifact = await publishArtifact(options);
  const path = join(options.directory, "objects", artifact.sha256);
  await writeFile(path, "MZ broken package");
  await expect(checkReleases(options.directory)).rejects.toThrow(
    "RELEASE_OBJECT_CORRUPT",
  );
  await expect(publishArtifact(options)).rejects.toThrow(
    "RELEASE_OBJECT_CORRUPT",
  );
  expect(await readFile(path, "utf8")).toBe("MZ broken package");
});
test("路径式版本、无效平台架构和错误包格式不能发布", async () => {
  const { source, options } = await setup();
  await expect(
    publishArtifact({ ...options, version: "../escape" }),
  ).rejects.toThrow("INVALID_RELEASE_ARTIFACT");
  await expect(
    publishArtifact({ ...options, platform: "ios", architecture: "x64" }),
  ).rejects.toThrow("INVALID_RELEASE_ARTIFACT");
  await writeFile(source, "not an installer");
  await expect(publishArtifact(options)).rejects.toThrow(
    "RELEASE_PACKAGE_FORMAT_MISMATCH",
  );
});
test("外部 URL 清单和过大清单失败关闭，维护锁不会被遗留", async () => {
  const { options } = await setup();
  await mkdir(options.directory, { recursive: true });
  const path = join(options.directory, "manifest.json");
  await writeFile(
    path,
    JSON.stringify({
      schemaVersion: 1,
      artifacts: [],
      url: "https://evil.test",
    }),
  );
  await expect(publishArtifact(options)).rejects.toThrow(
    "INVALID_RELEASE_MANIFEST",
  );
  await writeFile(path, " ".repeat(128 * 1024 + 1));
  await expect(publishArtifact(options)).rejects.toThrow(
    "INVALID_RELEASE_MANIFEST",
  );
  await writeFile(path, JSON.stringify({ schemaVersion: 1, artifacts: [] }));
  expect((await publishArtifact(options)).size).toBeGreaterThan(0);
});

test("维护检查不接受会被 JavaScript 自动滚日的时间", async () => {
  const { options } = await setup();
  await publishArtifact(options);
  const path = join(options.directory, "manifest.json");
  const value = JSON.parse(await readFile(path, "utf8"));
  for (const publishedAt of invalidTimes) {
    value.artifacts[0].publishedAt = publishedAt;
    await writeFile(path, JSON.stringify(value));
    await expect(checkReleases(options.directory)).rejects.toThrow(
      "INVALID_RELEASE_MANIFEST",
    );
  }
});
