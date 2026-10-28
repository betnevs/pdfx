import { useEffect, useRef, useState } from "react";

export interface PasswordPromptProps {
  /** Name of the file being opened, shown so the user knows what is locked. */
  fileName: string;
  /** Copy for a failed attempt; null while the first password is pending. */
  error: string | null;
  /** True while the backend is trying the password. */
  busy: boolean;
  onSubmit: (password: string) => void;
  onCancel: () => void;
}

/**
 * Modal that asks for a User Password.
 *
 * The Document is only opened, and pages only rendered, after the backend
 * accepts a password: the prompt itself never displays document content.
 */
export function PasswordPrompt({
  fileName,
  error,
  busy,
  onSubmit,
  onCancel,
}: PasswordPromptProps) {
  const [password, setPassword] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  return (
    <div className="password-overlay">
      <form
        className="password-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="password-title"
        onSubmit={(event) => {
          event.preventDefault();
          if (!busy) {
            onSubmit(password);
          }
        }}
        onKeyDown={(event) => {
          if (event.key === "Escape" && !busy) {
            event.preventDefault();
            onCancel();
          }
        }}
      >
        <h2 id="password-title">输入密码</h2>
        <p className="password-file" title={fileName}>
          {fileName}
        </p>
        <p className="password-hint">
          此 PDF 需要密码才能打开。密码只在本机用于解密，不会被保存。
        </p>
        {error && (
          <p className="password-error" role="alert">
            {error}
          </p>
        )}
        <label className="password-label" htmlFor="pdf-password">
          密码
        </label>
        <input
          id="pdf-password"
          ref={inputRef}
          type="password"
          value={password}
          disabled={busy}
          autoComplete="current-password"
          onChange={(event) => setPassword(event.target.value)}
        />
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
            打开
          </button>
        </div>
      </form>
    </div>
  );
}
