import type { DocumentView } from "./backend";

/** Visible explanation for a Permission-Restricted (read-only) document. */
export const READ_ONLY_REASON =
  "只读模式：文档所有者限制了编辑权限，修改、注释与页面操作均不可用。";

/** Save Changes must stay disabled for a read-only document. */
export const SAVE_CHANGES_READ_ONLY = "只读文档不能保存更改。";

/**
 * ADR-0007: saving an encrypted Source Document writes an unencrypted Output
 * Document. Ticket 10 must show this warning before Save Changes or Save As;
 * the warning is exposed here so the rule is testable before Save exists.
 */
export const UNENCRYPTED_OUTPUT_WARNING =
  "另存为会生成不加密的副本；输出文件不会保留打开密码或权限限制。";

/** The save and edit affordances a Document allows. */
export interface DocumentPolicy {
  readOnly: boolean;
  /** Why editing tools are unavailable, or null when the document is editable. */
  editingDisabledReason: string | null;
  /** Why Save Changes must stay disabled, or null when it may be enabled. */
  saveChangesDisabledReason: string | null;
  /** Warning to show before any save, or null when no warning is needed. */
  unencryptedOutputWarning: string | null;
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
  };
}
