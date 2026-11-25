import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { setLanguage } from "@assemble/display";
import { noticeText } from "@/lib/notice-text";
import en from "@/locales/text.en.json";
import zhCN from "@/locales/text.zh-CN.json";

beforeEach(() => setLanguage("zh-CN"));
afterEach(() => setLanguage("zh-CN"));

describe("应用内 Notice 的安全翻译", () => {
  test("所有已知固定句完整匹配，中文原文与英文语义对应", () => {
    for (const key of Object.keys(zhCN.notice.fixed) as Array<
      keyof typeof zhCN.notice.fixed
    >) {
      const notice = { kind: "info" as const, message: zhCN.notice.fixed[key] };
      expect(noticeText(notice)).toBe(notice.message);
      setLanguage("en");
      expect(noticeText(notice)).toBe(en.notice.fixed[key]);
      setLanguage("zh-CN");
    }
  });

  test("当前 session/send_panel/transfers/tray 的所有固定 Notice 均有翻译", () => {
    setLanguage("en");
    let checked = 0;
    for (const filename of [
      "session.rs",
      "send_panel.rs",
      "transfers.rs",
      "tray.rs",
    ]) {
      const source = readFileSync(
        new URL(`../../src-tauri/src/${filename}`, import.meta.url),
        "utf8",
      );
      for (const match of source.matchAll(
        /Notice::(info|error)\(\s*"([^"\r\n]*)"/g,
      )) {
        const kind = match[1] as "info" | "error";
        const message = noticeText({ kind, message: match[2] });
        expect(message).not.toBe(en.notice.unknownInfo);
        expect(message).not.toBe(en.notice.unknownError);
        expect(message).not.toMatch(/[\u3400-\u9fff]|notice\./);
        checked++;
      }
    }
    expect(checked).toBeGreaterThan(10);
  });

  test("未登录的发送和云盘上传分别翻译，不把任意 purpose 当作系统动作", () => {
    setLanguage("en");
    expect(
      noticeText({ kind: "info", message: "请先登录，再发送文件。" }),
    ).toBe("Sign in before sending files.");
    expect(
      noticeText({ kind: "info", message: "请先登录，再往云盘里存文件。" }),
    ).toBe("Sign in before uploading files to the cloud drive.");
    expect(
      noticeText({ kind: "info", message: "请先登录，再执行未知内部命令。" }),
    ).toBe(en.notice.unknownInfo);
  });

  test("过大和无法读取的文件名原样插值，包括中文、尖括号和嵌套括号", () => {
    for (const name of [
      "报告 Report.pdf",
      "季度<2026>&资料.txt",
      "「文件」超过 100 MB，发不出去。.txt",
      "{{name}}.txt",
    ]) {
      const large = {
        kind: "error" as const,
        message: `「${name}」超过 100 MB，发不出去。`,
      };
      const unreadable = {
        kind: "error" as const,
        message: `「${name}」读不出来，可能已被移走或没有权限。`,
      };
      setLanguage("zh-CN");
      expect(noticeText(large)).toBe(large.message);
      expect(noticeText(unreadable)).toBe(unreadable.message);
      setLanguage("en");
      expect(noticeText(large)).toBe(
        `“${name}” exceeds 100 MB and cannot be sent.`,
      );
      expect(noticeText(unreadable)).toBe(
        `“${name}” cannot be read. It may have been moved, or you may not have permission.`,
      );
      expect(large.message).toBe(`「${name}」超过 100 MB，发不出去。`);
    }
  });

  test("非完整模板、空文件名和额外诊断不能进入文件名插值分支", () => {
    setLanguage("en");
    for (const message of [
      "前缀「报告.pdf」超过 100 MB，发不出去。",
      "「报告.pdf」超过 100 MB，发不出去。\n",
      "「报告.pdf」超过 100 MB，发不出去。 internal-error",
      "「」超过 100 MB，发不出去。",
      "报告.pdf」超过 100 MB，发不出去。",
      "「报告.pdf」读不出来，可能已被移走或没有权限。 (database detail)",
    ])
      expect(noticeText({ kind: "error", message })).toBe(
        en.notice.unknownError,
      );
  });

  test("同一 Notice 随语言切换，原始 Rust 诊断在两种语言均安全兜底", () => {
    const notice = {
      kind: "error" as const,
      message: "暂时连不上服务，请检查网络后重试。",
    };
    expect(noticeText(notice)).toBe(notice.message);
    setLanguage("en");
    expect(noticeText(notice)).toBe(en.notice.fixed.unreachable);
    setLanguage("zh-CN");
    expect(noticeText(notice)).toBe(notice.message);
    for (const language of ["zh-CN", "en"] as const) {
      setLanguage(language);
      const dictionary = language === "en" ? en : zhCN;
      for (const message of [
        "raw error: token=private /private/path",
        "错误：SELECT password_hash FROM accounts",
        "constructor",
        "__proto__",
        "",
      ]) {
        expect(noticeText({ kind: "error", message })).toBe(
          dictionary.notice.unknownError,
        );
        expect(noticeText({ kind: "info", message })).toBe(
          dictionary.notice.unknownInfo,
        );
      }
    }
  });
});
