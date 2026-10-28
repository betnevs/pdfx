import type { DocumentView } from "./backend";

/** Visible explanation for a Permission-Restricted (read-only) document. */
export const READ_ONLY_REASON =
  "只读模式：文档所有者限制了编辑权限，修改、注释与页面操作均不可用。";

/** Save Changes must stay disabled for a read-only document. */
export const SAVE_CHANGES_READ_ONLY = "只读文档不能保存更改。";

/** Visible explanation when Permission Restrictions deny printing. */
export const PRINT_DISABLED_REASON = "文档所有者限制了打印权限。";

/**
 * ADR-0007: saving an encrypted Source Document writes an unencrypted Output
 * Document. Ticket 10 must show this warning before Save Changes or Save As;
 * the warning is exposed here so the rule is testable before Save exists.
 */
export const UNENCRYPTED_OUTPUT_WARNING =
  "另存为会生成不加密的副本；输出文件不会保留打开密码或权限限制。";

/** Dialog title and labels for the save flow's opening-password option. */
export const SAVE_OPTIONS_TITLE = "保存选项";
export const SAVE_PASSWORD_OPTION_LABEL = "设置打开密码";
export const SAVE_PASSWORD_LABEL = "打开密码";
export const SAVE_PASSWORD_CONFIRM_LABEL = "确认密码";

/** Validation copy shown inside the save-options dialog. */
export const SAVE_PASSWORD_REQUIRED = "请输入打开密码";
export const SAVE_PASSWORD_MISMATCH = "两次输入的密码不一致";
/**
 * The AES-256 security handler hashes the UTF-8 password truncated to 127
 * bytes (ISO 32000-2 Algorithm 2.A). The dialog rejects longer values rather
 * than let the backend silently ignore the tail.
 */
export const SAVE_PASSWORD_MAX_BYTES = 127;
export const SAVE_PASSWORD_TOO_LONG = "密码过长：UTF-8 编码后最多 127 字节。";

/**
 * Heading shown when saving without a password would write an unencrypted
 * Output Document. ADR-0007 requires the user to accept that explicitly; the
 * primary action spells the consequence out ("继续保存（不加密）").
 */
export const UNENCRYPTED_OUTPUT_HEADING = "输出将不加密";

/** Explanation when the current Output Document already carries a password. */
export const PROTECTED_OUTPUT_WARNING =
  "当前输出文件已设置打开密码；不设置新密码将写入不加密的文件。";

/** The save and edit affordances a Document allows. */
export interface DocumentPolicy {
  readOnly: boolean;
  /** Why editing tools are unavailable, or null when the document is editable. */
  editingDisabledReason: string | null;
  /** Why Save Changes must stay disabled, or null when it may be enabled. */
  saveChangesDisabledReason: string | null;
  /** Warning to show before any save, or null when no warning is needed. */
  unencryptedOutputWarning: string | null;
  /** Why printing is unavailable, or null when the document may print. */
  printDisabledReason: string | null;
}

/**
 * Derives the edit/save policy for an open Document.
 *
 * The backend computes `readOnly` from the security handler's flags; this
 * helper only turns that state and `encrypted` into the copy later tickets
 * (editing and save) must display. Individual editing tools should consult
 * `document.permissions` for finer gating, not `readOnly` alone.
 */
export function documentPolicy(document: DocumentView): DocumentPolicy {
  return {
    readOnly: document.readOnly,
    editingDisabledReason: document.readOnly ? READ_ONLY_REASON : null,
    saveChangesDisabledReason: document.readOnly ? SAVE_CHANGES_READ_ONLY : null,
    unencryptedOutputWarning: document.encrypted ? UNENCRYPTED_OUTPUT_WARNING : null,
    printDisabledReason: document.permissions.canPrint ? null : PRINT_DISABLED_REASON,
  };
}
