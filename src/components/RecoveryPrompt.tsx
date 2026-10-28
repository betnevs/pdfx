import { useEffect, useRef } from "react";

import type { RecoverySnapshotView } from "../lib/backend";

export interface RecoveryPromptProps {
  /** Recovery Files found at start-up, newest first. */
  candidates: RecoverySnapshotView[];
  /** Session whose restore or discard is running, if any. */
  busySessionId: string | null;
  /** Why the last restore or discard failed. */
  error: string | null;
  onRestore: (sessionId: string) => void;
  onDiscard: (sessionId: string) => void;
  /** Keeps the files on disk and hides the prompt until the next start. */
  onDismiss: () => void;
}

function recoveryTime(millis: number): string {
  const date = new Date(millis);
  if (Number.isNaN(date.getTime())) {
    return "";
  }
  return date.toLocaleString("zh-CN", { hour12: false });
}

/**
 * Start-up question about Recovery Files left by an abnormal exit.
 *
 * Restoring opens the snapshot as the Document; the original Source Document
 * is never written. Discarding deletes that snapshot. Dismissing keeps every
 * file for the next start, so a recovery decision is never made silently.
 */
export function RecoveryPrompt({
  candidates,
  busySessionId,
  error,
  onRestore,
  onDiscard,
  onDismiss,
}: RecoveryPromptProps) {
  const dismissRef = useRef<HTMLButtonElement>(null);
  const busy = busySessionId !== null;

  useEffect(() => {
    dismissRef.current?.focus();
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) {
        event.preventDefault();
        onDismiss();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [busy, onDismiss]);

  return (
    <div className="password-overlay">
      <div
        className="password-dialog recovery-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="recovery-title"
        aria-describedby="recovery-description"
        aria-busy={busy}
      >
        <h2 id="recovery-title">恢复未保存的文档？</h2>
        <p className="password-hint" id="recovery-description">
          PDFX 上次可能未正常退出。以下恢复文件包含尚未保存的编辑。恢复后原文件不会被修改，保存时会写入输出文件。
        </p>

        <ul className="recovery-list">
          {candidates.map((candidate) => {
            const candidateBusy = busySessionId === candidate.sessionId;
            return (
              <li className="recovery-item" key={candidate.sessionId}>
                <div className="recovery-info">
                  <span className="recovery-name" title={candidate.displayName}>
                    {candidate.displayName}
                  </span>
                  <span className="recovery-path" title={candidate.sourcePath}>
                    {candidate.sourcePath}
                  </span>
                  <span className="recovery-meta">
                    {recoveryTime(candidate.createdAtMillis)}
                    {candidate.sourceEncrypted
                      ? " · 原文件已加密；恢复副本未加密，保存时仍会要求确认。"
                      : " · 未保存的编辑"}
                  </span>
                </div>
                <div className="recovery-actions">
                  <button
                    type="button"
                    className="secondary-button"
                    aria-label={`丢弃 ${candidate.displayName}`}
                    disabled={busy}
                    onClick={() => onDiscard(candidate.sessionId)}
                  >
                    {candidateBusy ? "正在丢弃…" : "丢弃"}
                  </button>
                  <button
                    type="button"
                    className="open-button"
                    aria-label={`恢复 ${candidate.displayName}`}
                    disabled={busy}
                    onClick={() => onRestore(candidate.sessionId)}
                  >
                    {candidateBusy ? "正在恢复…" : "恢复"}
                  </button>
                </div>
              </li>
            );
          })}
        </ul>

        {error && (
          <p className="password-error" role="alert">
            {error}
          </p>
        )}

        <div className="password-actions">
          <button
            ref={dismissRef}
            type="button"
            className="secondary-button"
            onClick={onDismiss}
            disabled={busy}
          >
            稍后处理
          </button>
        </div>
      </div>
    </div>
  );
}
