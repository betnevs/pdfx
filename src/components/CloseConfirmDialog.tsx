import { useEffect, useRef } from "react";

export interface CloseConfirmDialogProps {
  /** Name of the Document whose unsaved changes would be lost. */
  documentName: string;
  /** True while the close-triggered save is running. */
  busy: boolean;
  /** Why the last save attempt did not close the window, if it failed. */
  message: string | null;
  onSave: () => void;
  onDiscard: () => void;
  onCancel: () => void;
}

/**
 * Confirmation shown when the OS asks a window with unsaved changes to close.
 *
 * The three choices map to the spec: Save writes through the normal save
 * pipeline and closes only on success, Don't Save discards the in-memory
 * changes and closes, Cancel keeps the window open. Escape cancels; focus
 * starts on Cancel so the safe choice is one keypress away.
 */
export function CloseConfirmDialog({
  documentName,
  busy,
  message,
  onSave,
  onDiscard,
  onCancel,
}: CloseConfirmDialogProps) {
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    cancelRef.current?.focus();
  }, []);

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
        className="password-dialog close-confirm-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="close-confirm-title"
        aria-describedby="close-confirm-description"
        aria-busy={busy}
      >
        <h2 id="close-confirm-title">关闭前保存更改？</h2>
        <p className="password-file" title={documentName}>
          {documentName}
        </p>
        <p className="password-hint" id="close-confirm-description">
          该文档有未保存的更改。关闭窗口前要保存吗？
        </p>
        {message && (
          <p className="password-error" role="alert">
            {message}
          </p>
        )}
        <div className="password-actions">
          <button
            ref={cancelRef}
            type="button"
            className="secondary-button"
            onClick={onCancel}
            disabled={busy}
          >
            取消
          </button>
          <button
            type="button"
            className="secondary-button"
            onClick={onDiscard}
            disabled={busy}
          >
            不保存
          </button>
          <button
            type="button"
            className="open-button"
            onClick={onSave}
            disabled={busy}
          >
            {busy ? "正在保存…" : "保存"}
          </button>
        </div>
      </div>
    </div>
  );
}
