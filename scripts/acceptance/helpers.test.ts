import { describe, expect, test } from "bun:test";
import { mkdtemp, utimes, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  clock,
  DEFAULT_ACCOUNT,
  DEFAULT_TIMEOUT_SECONDS,
  findLanded,
  formatSize,
  isNumberedName,
  markerLines,
  numberedName,
  parseArguments,
  randomFile,
  sha256,
  sha256File,
  splitExtension,
} from "./helpers";

describe("parseArguments", () => {
  test("不带参数就是起环境，账户与等待时限取默认值", () => {
    expect(parseArguments([])).toEqual({
      command: "start",
      account: DEFAULT_ACCOUNT,
      timeout: DEFAULT_TIMEOUT_SECONDS,
    });
  });

  test("stop 单独一个词", () => {
    expect(parseArguments(["stop"])).toMatchObject({ command: "stop" });
    expect(parseArguments(["stop", "extra"])).toContain("stop 后面");
  });

  test("账户不分大小写，时限是整数", () => {
    expect(parseArguments(["--account", "Me.Alex", "--timeout", "45"])).toEqual(
      {
        command: "start",
        account: "me.alex",
        timeout: 45,
      },
    );
  });

  test("不合法的参数得到一段说明，而不是悄悄当成默认值", () => {
    for (const argv of [
      ["--account"],
      ["--account", ""],
      ["--account", "ab"],
      ["--account", "中文名字"],
      ["--account", "ok", "--timeout", "0"],
      ["--account", "ok", "--timeout", "3601"],
      ["--account", "ok", "--timeout", "abc"],
      ["--unknown", "x"],
      ["start"],
    ]) {
      expect(parseArguments(argv)).toContain("用法：");
    }
  });
});

describe("markerLines", () => {
  test("只认行首的标记，标记后紧跟制表符或行尾", () => {
    const text = [
      '给 [send_offered] {"id":"x"}',
      "peer-online\t8e66\t工作电脑",
      "    文件 peer-online\t 不是行首",
      "peer-onlineX\t冒牌",
      "peer-offline",
      "received\t合同.pdf\t12\tabc\t/tmp/合同.pdf",
      "received\t合同 (2).pdf\t12\tabc\t/tmp/x",
    ].join("\n");
    expect(markerLines(text, "peer-online")).toEqual([["8e66", "工作电脑"]]);
    expect(markerLines(text, "peer-offline")).toEqual([[]]);
    expect(markerLines(text, "received")).toEqual([
      ["合同.pdf", "12", "abc", "/tmp/合同.pdf"],
      ["合同 (2).pdf", "12", "abc", "/tmp/x"],
    ]);
    expect(markerLines(text, "send-settled")).toEqual([]);
    expect(markerLines("", "online")).toEqual([]);
  });

  test("提醒的标记：标识与到点时刻、通知生成的时刻、办完了", () => {
    const text = [
      "已新建提醒「看看 Assemble 有没有弹提醒」（3fd8），01:40 到点。",
      "reminder\t3fd85bc3\t1795542026330",
      "notification\tn-3fd85bc3\t1795542033308",
      "reminder-closed\t3fd85bc3",
    ].join("\n");
    expect(markerLines(text, "reminder")).toEqual([
      ["3fd85bc3", "1795542026330"],
    ]);
    expect(markerLines(text, "notification")).toEqual([
      ["n-3fd85bc3", "1795542033308"],
    ]);
    expect(markerLines(text, "reminder-closed")).toEqual([["3fd85bc3"]]);
  });
});

describe("clock", () => {
  test("本地时刻写成 时:分", () => {
    expect(clock(new Date(2026, 10, 25, 9, 5).getTime())).toBe("09:05");
    expect(clock(new Date(2026, 10, 25, 23, 0).getTime())).toBe("23:00");
  });
});

describe("撞名加序号", () => {
  test("主干与扩展名的分法与接收时一致", () => {
    expect(splitExtension("报告.pdf")).toEqual({
      stem: "报告",
      extension: ".pdf",
    });
    expect(splitExtension("归档.tar.gz")).toEqual({
      stem: "归档",
      extension: ".tar.gz",
    });
    expect(splitExtension("备份.TAR.GZ")).toEqual({
      stem: "备份",
      extension: ".TAR.GZ",
    });
    expect(splitExtension(".profile")).toEqual({
      stem: ".profile",
      extension: "",
    });
    expect(splitExtension("README")).toEqual({ stem: "README", extension: "" });
    expect(splitExtension(`a.${"x".repeat(65)}`)).toEqual({
      stem: `a.${"x".repeat(65)}`,
      extension: "",
    });
  });

  test("序号加在主干之后、扩展名之前", () => {
    expect(numberedName("报告.pdf", 2)).toBe("报告 (2).pdf");
    expect(numberedName("归档.tar.gz", 3)).toBe("归档 (3).tar.gz");
    expect(numberedName("README", 2)).toBe("README (2)");
  });

  test("认得出撞名后的样子，也认得出不是的", () => {
    expect(isNumberedName("报告 (2).pdf", "报告.pdf")).toBe(true);
    expect(isNumberedName("报告 (12).pdf", "报告.pdf")).toBe(true);
    expect(isNumberedName("README (2)", "README")).toBe(true);
    // 原名不算撞名；(1) 不会出现；序号必须在主干与扩展名之间。
    expect(isNumberedName("报告.pdf", "报告.pdf")).toBe(false);
    expect(isNumberedName("报告 (1).pdf", "报告.pdf")).toBe(false);
    expect(isNumberedName("报告 (2) 副本.pdf", "报告.pdf")).toBe(false);
    expect(isNumberedName("报告 (2).png", "报告.pdf")).toBe(false);
    expect(isNumberedName("周报 (2).pdf", "报告.pdf")).toBe(false);
    expect(isNumberedName("报告(2).pdf", "报告.pdf")).toBe(false);
    expect(isNumberedName("归档 (2).tar.gz", "归档.tar.gz")).toBe(true);
    expect(isNumberedName("归档.tar (2).gz", "归档.tar.gz")).toBe(false);
  });
});

describe("摘要与落盘", () => {
  test("sha256 是十六进制小写", async () => {
    const expected =
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    expect(sha256(new TextEncoder().encode("abc"))).toBe(expected);
    const directory = await mkdtemp(join(tmpdir(), "assemble-helpers-"));
    const path = join(directory, "abc");
    await writeFile(path, "abc");
    expect(await sha256File(path)).toBe(expected);
    await expect(sha256File(join(directory, "missing"))).rejects.toThrow();
  });

  test("随机文件各不相同，摘要与内容对得上", async () => {
    const first = randomFile(1024);
    const second = randomFile(1024);
    expect(first.bytes).toHaveLength(1024);
    expect(first.sha256).not.toBe(second.sha256);
    const directory = await mkdtemp(join(tmpdir(), "assemble-helpers-"));
    const path = join(directory, "random");
    await writeFile(path, first.bytes);
    expect(await sha256File(path)).toBe(first.sha256);
  });

  test("只认刚落盘的、摘要对得上的那个", async () => {
    const directory = await mkdtemp(join(tmpdir(), "assemble-helpers-"));
    const content = new TextEncoder().encode("同一个文件");
    const digest = sha256(content);
    const past = new Date("2026-01-01T00:00:00Z");

    // 上一次运行留下的同名文件：内容一样，但不是这次落的盘。
    await writeFile(join(directory, "报告.pdf"), content);
    await utimes(join(directory, "报告.pdf"), past, past);
    const since = Date.now() - 1_000;
    expect(await findLanded(directory, "报告.pdf", digest, since)).toBeNull();

    // 这次的落成了撞名后的样子。
    await writeFile(join(directory, "报告 (2).pdf"), content);
    expect(await findLanded(directory, "报告.pdf", digest, since)).toEqual({
      path: join(directory, "报告 (2).pdf"),
      sha256: digest,
    });

    // 内容不对的不算；摘要不对时找不到，而不是随便挑一个同名文件。
    await writeFile(join(directory, "报告 (3).pdf"), "别的文件");
    expect(
      await findLanded(directory, "报告.pdf", sha256(content), since),
    ).toEqual({
      path: join(directory, "报告 (2).pdf"),
      sha256: digest,
    });
    expect(
      await findLanded(directory, "报告.pdf", "0".repeat(64), since),
    ).toBeNull();
    expect(await findLanded(directory, "合同.pdf", digest, since)).toBeNull();
    expect(
      await findLanded(
        join(directory, "没有这个目录"),
        "报告.pdf",
        digest,
        since,
      ),
    ).toBeNull();
  });

  test("大小按 MB 或字节给人看", () => {
    expect(formatSize(612)).toBe("612 字节");
    expect(formatSize(2 * 1024 * 1024)).toBe("2.0 MB");
    expect(formatSize(1_500_000)).toBe("1.4 MB");
  });
});
