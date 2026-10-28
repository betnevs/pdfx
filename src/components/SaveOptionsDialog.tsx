import { useEffect, useRef, useState } from "react";

import {
  PROTECTED_OUTPUT_WARNING,
  SAVE_OPTIONS_TITLE,
  SAVE_PASSWORD_CONFIRM_LABEL,
  SAVE_PASSWORD_LABEL,
  SAVE_PASSWORD_MISMATCH,
  SAVE_PASSWORD_OPTION_LABEL,
  SAVE_PASSWORD_REQUIRED,
  UNENCRYPTED_OUTPUT_HEADING,
  UNENCRYPTED_OUTPUT_WARNING,
} from "../lib/document-policy";

export interface SaveOptionsDialogProps {
  /** Name of the document being saved, so the consequence is unambiguous. */
  documentName: string;
  busy: boolean;
  /** An unencrypted output of an encrypted Source needs ADR-0007 acceptance. */
  sourceEncrypted: boolean;
  /** Saving without a password would remove the current output's protection. */
  outputEncrypted: boolean;
  /** Show the password fields straight away (a retry after a password save). */
  initialProtect?: boolean;
  /**
   * Called with the chosen opening password, or null to write an unencrypted
   * output. The password lives only in this component until submit.
   */
  onSave: (password: string | null) => void;
  onCancel: () => void;
}

/**
 * Options shown for every Save As and for Save Changes that touches
 * protection: the user may set an opening password, or explicitly accept an
 * unencrypted output. The dialog itself is the ADR-0007 confirmation when the
 * Source Document (or the current Output Document) is encrypted.
 */
export function SaveOptionsDialog({
  documentName,
  busy,
  sourceEncrypted,
  outputEncrypted,
  initialProtect = false,
  onSave,
  onCancel,
}: SaveOptionsDialogProps) {
  const [protect, setProtect] = useState(initialProtect);
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [validation, setValidation] = useState<string | null>(null);
  const passwordRef = useRef<HTMLInputElement>(null);

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

  const clearSecret = () => {
    setPassword("");
    setConfirmation("");
  };

  const needsUnencryptedDecision =
    !protect && (sourceEncrypted || outputEncrypted);

  const submit = () => {
    if (busy) {
      return;
    }
    if (!protect) {
      onSave(null);
      return;
    }
    if (password.length === 0) {
      setValidation(SAVE_PASSWORD_REQUIRED);
      passwordRef.current?.focus();
      return;
    }
    if (password !== confirmation) {
      setValidation(SAVE_PASSWORD_MISMATCH);
      return;
    }
    // Clear the secret before handing it to the save flow; the request keeps
    // it only for the in-flight write.
    const chosen = password;
    clearSecret();
    setProtect(false);
    setValidation(null);
    onSave(chosen);
  };

  return (
    <div className="password-overlay">
      <form
        className="password-dialog save-options-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="save-options-title"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <h2 id="save-options-title">{SAVE_OPTIONS_TITLE}</h2>
        <p className="password-file" title={documentName}>
          {documentName}
        </p>
        <label className="save-options-checkbox">
          <input
            type="checkbox"
            checked={protect}
            disabled={busy}
            onChange={(event) => {
              setProtect(event.target.checked);
              setValidation(null);
              if (!event.target.checked) {
                clearSecret();
              }
            }}
          />
          {SAVE_PASSWORD_OPTION_LABEL}
        </label>
        {protect ? (
          <>
            <label className="password-label" htmlFor="save-password">
              {SAVE_PASSWORD_LABEL}
            </label>
            <input
              id="save-password"
              ref={passwordRef}
              type="password"
              value={password}
              disabled={busy}
              autoComplete="new-password"
              onChange={(event) => setPassword(event.target.value)}
            />
            <label className="password-label" htmlFor="save-password-confirm">
              {SAVE_PASSWORD_CONFIRM_LABEL}
            </label>
            <input
              id="save-password-confirm"
              type="password"
              value={confirmation}
              disabled={busy}
              autoComplete="new-password"
              onChange={(event) => setConfirmation(event.target.value)}
            />
            {validation && (
              <p className="password-error" role="alert">
                {validation}
              </p>
            )}
          </>
        ) : needsUnencryptedDecision ? (
          <div className="save-options-warning">
            <p className="save-options-warning-title">
              {UNENCRYPTED_OUTPUT_HEADING}
            </p>
            {outputEncrypted && (
              <p className="password-hint">{PROTECTED_OUTPUT_WARNING}</p>
            )}
            {sourceEncrypted && (
              <p className="password-hint">
                {UNENCRYPTED_OUTPUT_WARNING} 确定继续保存吗？
              </p>
            )}
          </div>
        ) : null}
        <div className="password-actions">
          <button
            type="button"
            className="secondary-button"
            onClick={onCancel}
            disabled={busy}
          >
            取消
          </button>
          <button type="submit" className="open-button" disabled={busy}>
            {protect
              ? "加密保存"
              : needsUnencryptedDecision
                ? "继续保存（不加密）"
                : "保存"}
          </button>
        </div>
      </form>
    </div>
  );
}
