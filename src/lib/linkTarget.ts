import {
  toCommandError,
  type CommandErrorCode,
  type LinkTarget,
  type LinkUnsupportedReason,
} from "./backend";

/** Matches the Rust command boundary's URL length cap. */
export const MAX_LINK_URL_LENGTH = 2048;

/** The result of validating one field of the link dialog. */
export type LinkTargetValidation =
  | { ok: true; target: LinkTarget }
  | { ok: false; error: string };

/**
 * Validates an external link URL and trims the surrounding whitespace.
 *
 * The rules mirror the Rust command boundary so a typo is explained in the
 * dialog before any command runs: `http://`, `https://` and `mailto:` only,
 * no internal whitespace, a host for web links and an `@` for mail links.
 */
export function validateLinkUrl(input: string): LinkTargetValidation {
  const url = input.trim();
  if (url.length === 0) {
    return { ok: false, error: "请输入链接网址。" };
  }
  if (url.length > MAX_LINK_URL_LENGTH) {
    return {
      ok: false,
      error: `链接网址过长（最多 ${MAX_LINK_URL_LENGTH} 个字符）。`,
    };
  }
  if (/[\s\u0000-\u001f\u007f]/.test(url)) {
    return { ok: false, error: "链接网址不能包含空格。" };
  }

  const lower = url.toLowerCase();
  if (lower.startsWith("http://") || lower.startsWith("https://")) {
    const rest = url.slice(lower.startsWith("http://") ? 7 : 8);
    const host = rest.split(/[/?#]/)[0] ?? "";
    if (host.length === 0 || host.startsWith(":")) {
      return { ok: false, error: "链接网址缺少主机名。" };
    }
  } else if (lower.startsWith("mailto:")) {
    const address = url.slice(7);
    if (address.length === 0 || !address.includes("@")) {
      return { ok: false, error: "邮件链接缺少收件人地址。" };
    }
  } else {
    return {
      ok: false,
      error: "链接网址必须以 http://、https:// 或 mailto: 开头。",
    };
  }

  return { ok: true, target: { kind: "url", url } };
}

/**
 * Validates the dialog's one-based page number and returns the zero-based
 * index `LinkTarget` expects. Only plain digits are accepted.
 */
export function validateLinkPage(
  input: string,
  pageCount: number,
): LinkTargetValidation {
  const text = input.trim();
  if (text.length === 0) {
    return { ok: false, error: "请输入页码。" };
  }
  if (!/^\d+$/.test(text)) {
    return { ok: false, error: "页码必须是整数。" };
  }
  const page = Number.parseInt(text, 10);
  if (page < 1 || page > pageCount) {
    return {
      ok: false,
      error: `页码必须在 1 到 ${pageCount} 之间。`,
    };
  }
  return { ok: true, target: { kind: "page", pageIndex: page - 1 } };
}

/** Short target description shown in the link toolbar. */
export function linkTargetLabel(target: LinkTarget): string {
  return target.kind === "url" ? target.url : `第 ${target.pageIndex + 1} 页`;
}

/** Chinese explanation for a foreign link PDFX must leave untouched. */
export function linkUnsupportedText(reason: LinkUnsupportedReason): string {
  switch (reason) {
    case "action":
      return "此链接使用 JavaScript 等 PDFX 不支持的操作，无法编辑。";
    case "destination":
      return "此链接指向的目标位置无法解析，无法编辑。";
    case "malformed":
      return "此链接缺少可读取的目标，无法编辑。";
  }
}

/** Command failures specific to links, in Chinese. */
const LINK_ERROR_MESSAGES: Partial<Record<CommandErrorCode, string>> = {
  invalid_argument: "链接目标无效，请检查网址或页码。",
  page_out_of_range: "目标页码超出范围。",
  permission_denied: "文档所有者限制了编辑权限。",
  read_only_document: "此文档不允许编辑内容。",
  unsupported_link: "此链接无法编辑，已保持原样。",
  document_not_found: "文档已关闭，请重新打开。",
};

/** Maps a link command failure to Chinese guidance for the dialog or notice. */
export function linkErrorText(error: unknown): string {
  const commandError = toCommandError(error);
  return LINK_ERROR_MESSAGES[commandError.code] ?? commandError.message;
}
