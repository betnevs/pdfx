import { describe, expect, it } from "vitest";

import {
  linkErrorText,
  linkTargetLabel,
  linkUnsupportedText,
  validateLinkPage,
  validateLinkUrl,
} from "./linkTarget";

describe("validateLinkUrl", () => {
  it.each([
    "https://example.com",
    "https://example.com/docs?q=1#part",
    "http://example.com/a",
    "HTTPS://Example.com/Path",
    "mailto:team@example.com",
  ])("accepts %s unchanged after trimming", (url) => {
    expect(validateLinkUrl(`  ${url}  `)).toEqual({
      ok: true,
      target: { kind: "url", url },
    });
  });

  it.each([
    ["", "请输入链接网址。"],
    ["   ", "请输入链接网址。"],
    ["example.com", "链接网址必须以 http://、https:// 或 mailto: 开头。"],
    ["ftp://example.com", "链接网址必须以 http://、https:// 或 mailto: 开头。"],
    ["javascript:alert(1)", "链接网址必须以 http://、https:// 或 mailto: 开头。"],
    ["https://", "链接网址缺少主机名。"],
    ["https:///path", "链接网址缺少主机名。"],
    ["mailto:team", "邮件链接缺少收件人地址。"],
    ["https://example.com/a b", "链接网址不能包含空格。"],
  ])("rejects %j with a Chinese reason", (url, error) => {
    expect(validateLinkUrl(url)).toEqual({ ok: false, error });
  });

  it("rejects a URL longer than 2048 characters", () => {
    expect(validateLinkUrl(`https://example.com/${"a".repeat(2048)}`)).toEqual({
      ok: false,
      error: "链接网址过长（最多 2048 个字符）。",
    });
  });
});

describe("validateLinkPage", () => {
  it.each([
    ["1", 0],
    ["3", 2],
    [" 10 ", 9],
  ])("accepts page %j as zero-based index %i", (input, pageIndex) => {
    expect(validateLinkPage(input, 10)).toEqual({
      ok: true,
      target: { kind: "page", pageIndex },
    });
  });

  it.each([
    ["", "请输入页码。"],
    ["abc", "页码必须是整数。"],
    ["2.5", "页码必须是整数。"],
    ["1e2", "页码必须是整数。"],
    ["-1", "页码必须是整数。"],
    ["0", "页码必须在 1 到 10 之间。"],
    ["11", "页码必须在 1 到 10 之间。"],
  ])("rejects page %j with a Chinese reason", (input, error) => {
    expect(validateLinkPage(input, 10)).toEqual({ ok: false, error });
  });
});

describe("link labels", () => {
  it("labels a URL target with the URL and a page target with its number", () => {
    expect(linkTargetLabel({ kind: "url", url: "https://example.com" })).toBe(
      "https://example.com",
    );
    expect(linkTargetLabel({ kind: "page", pageIndex: 2 })).toBe("第 3 页");
  });

  it.each([
    ["action", "此链接使用 JavaScript 等 PDFX 不支持的操作，无法编辑。"],
    ["destination", "此链接指向的目标位置无法解析，无法编辑。"],
    ["malformed", "此链接缺少可读取的目标，无法编辑。"],
  ] as const)("explains why a %s link cannot be edited", (reason, text) => {
    expect(linkUnsupportedText(reason)).toBe(text);
  });

  it("maps link command errors to Chinese guidance", () => {
    expect(
      linkErrorText({ code: "invalid_argument", message: "invalid link URL" }),
    ).toBe("链接目标无效，请检查网址或页码。");
    expect(
      linkErrorText({ code: "page_out_of_range", message: "page out of range" }),
    ).toBe("目标页码超出范围。");
    expect(
      linkErrorText({ code: "permission_denied", message: "denied" }),
    ).toBe("文档所有者限制了编辑权限。");
    expect(
      linkErrorText({ code: "unsupported_link", message: "unsupported" }),
    ).toBe("此链接无法编辑，已保持原样。");
    expect(
      linkErrorText({ code: "internal", message: "链接层失败" }),
    ).toBe("链接层失败");
  });
});
