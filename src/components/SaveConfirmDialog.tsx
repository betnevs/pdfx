import { useEffect } from "react";

import { UNENCRYPTED_OUTPUT_WARNING } from "../lib/document-policy";

export interface SaveConfirmDialogProps {
  /** Name of the document being saved, so the consequence is unambiguous. */
  documentName: string;
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

/**
 * Confirmation shown before saving an encrypted Source Document.
 *
 * ADR-0007: the Output Document is always unencrypted, so the user must accept
 * that consequence explicitly before the pipeline writes anything. Cancelling
 * leaves both the Source Document and the previous Output Document untouched.
 */
export function SaveConfirmDialog({
  documentName,
  busy,
  onConfirm,
  onCancel,
}: SaveConfirmDialogProps) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) {
        event.preventDefault();
        onCancel();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [busy, onCancel]);

  return (
    <div className="password-overlay">
      <div
        className="password-dialog save-confirm-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="save-confirm-title"
      >
        <h2 id="save-confirm-title">输出将不加密</h2>
        <p className="password-file" title={documentName}>
          {documentName}
        </p>
        <p className="password-hint">
          {UNENCRYPTED_OUTPUT_WARNING} 确定继续保存吗？
        </p>
        <div className="password-actions">
          <button
            type="button"
            className="secondary-button"
            onClick={onCancel}
            disabled={busy}
          >
            取消
          </button>
          <button
            type="button"
            className="open-button"
            onClick={onConfirm}
            disabled={busy}
          >
            继续保存（不加密）
          </button>
        </div>
      </div>
    </div>
  );
}
