import type { DocumentView } from "../lib/backend";
import { documentPolicy, READ_ONLY_REASON } from "../lib/document-policy";

const EDITING_LATER = "编辑工具将在后续版本中提供。";
const SAVE_CHANGES_LATER = "保存功能将在后续版本中提供。";
const SAVE_AS_LATER = "另存为将在后续版本中提供。";

/**
 * Toolbar placeholders for the editing and save entries.
 *
 * Editing and saving arrive in later tickets. This component establishes the
 * contract they must honor now: a Permission-Restricted Document shows a
 * visible reason, and the entries are disabled and reference it. For encrypted
 * documents the unencrypted-output consequence is visible before any save
 * exists (ADR-0007); ticket 10 must gate Save Changes on `readOnly` and keep
 * this warning in its save flow.
 */
export function DocumentAccess({ document }: { document: DocumentView }) {
  const policy = documentPolicy(document);
  const reasonId = document.readOnly ? "read-only-reason" : undefined;

  return (
    <div className="document-access">
      {document.readOnly && (
        <p className="read-only-notice" id="read-only-reason">
          {READ_ONLY_REASON}
        </p>
      )}
      <div className="editing-tools" role="group" aria-label="编辑工具">
        <button
          type="button"
          disabled
          aria-describedby={reasonId}
          title={policy.editingDisabledReason ?? EDITING_LATER}
        >
          编辑工具（即将提供）
        </button>
        <button
          type="button"
          disabled
          aria-describedby={reasonId}
          title={policy.saveChangesDisabledReason ?? SAVE_CHANGES_LATER}
        >
          保存更改
        </button>
        <button
          type="button"
          disabled
          aria-describedby={reasonId}
          title={policy.unencryptedOutputWarning ?? SAVE_AS_LATER}
        >
          另存为…
        </button>
      </div>
      {policy.unencryptedOutputWarning && (
        <p className="save-warning" id="unencrypted-output-warning">
          {policy.unencryptedOutputWarning}
        </p>
      )}
    </div>
  );
}
