import { useState } from "react";

import {
  addAttachment,
  chooseAttachmentDestination,
  chooseAttachmentFile,
  deleteAttachment,
  openAttachment,
  saveAttachmentAs,
  toCommandError,
  type AttachmentView,
  type CommandErrorCode,
  type EditHistoryView,
} from "../lib/backend";

/** Formats a byte count for the attachment list. */
export function formatAttachmentSize(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  const kilobytes = bytes / 1024;
  if (kilobytes < 1024) {
    return `${kilobytes.toFixed(kilobytes < 10 ? 1 : 0)} KB`;
  }
  const megabytes = kilobytes / 1024;
  return `${megabytes.toFixed(megabytes < 10 ? 1 : 0)} MB`;
}

const ATTACHMENT_ERROR_MESSAGES: Partial<Record<CommandErrorCode, string>> = {
  file_not_found: "找不到文件，它可能已被移动或删除。",
  file_read_failed: "无法读取该文件。",
  permission_denied: "当前文档的权限不允许修改附件。",
  document_not_found: "文档已关闭。",
  attachment_too_large: "附件过大，单个附件不能超过 100 MB。",
  attachment_not_found: "该附件已不在文档中。",
  attachment_open_confirmation_required: "打开附件前需要确认。",
  attachment_open_failed: "无法用系统默认程序打开该附件。",
  attachment_target_is_source: "不能覆盖源文档，请选择其他位置。",
  attachment_target_is_output: "不能覆盖已保存的输出文档，请选择其他位置。",
  attachment_save_failed: "无法保存该附件。",
};

/** Chinese copy for an attachment command failure. */
export function attachmentErrorMessage(code: CommandErrorCode): string {
  return ATTACHMENT_ERROR_MESSAGES[code] ?? "附件操作失败，请重试。";
}

/** The unsafe-attachment warning shown before handing bytes to the OS. */
export const ATTACHMENT_OPEN_WARNING =
  "附件可能包含恶意内容或病毒。只有在你信任该附件的来源时才打开。";

interface Confirmation {
  kind: "open" | "delete";
  attachment: AttachmentView;
}

export interface AttachmentPanelProps {
  documentId: string;
  attachments: AttachmentView[];
  loading?: boolean;
  error?: string | null;
  /** Permission to add or delete attachments. */
  canModify: boolean;
  /** Permission Restrictions deny every editing capability. */
  readOnly: boolean;
  /** Feeds the unified history so the dirty marker and menu update. */
  onHistory: (history: EditHistoryView) => void;
  /** Asks the owner to refetch after a mutation. */
  onChanged: () => void;
}

/**
 * Document-wide attachment list backed by the PDF's `/EmbeddedFiles` name
 * tree.
 *
 * An attachment is never opened automatically: both opening (OS default
 * handler) and deleting ask for confirmation first, and both are Chinese so
 * the warning is visible to the user. Adding and deleting are undoable; the
 * returned history updates the save state and the native menu.
 */
export function AttachmentPanel({
  documentId,
  attachments,
  loading = false,
  error = null,
  canModify,
  readOnly,
  onHistory,
  onChanged,
}: AttachmentPanelProps) {
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const [busy, setBusy] = useState<"add" | "open" | "delete" | "save" | null>(
    null,
  );
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  async function addFile() {
    if (!canModify || busy) {
      return;
    }
    setActionError(null);
    setNotice(null);
    let path: string | null;
    try {
      path = await chooseAttachmentFile();
    } catch (error) {
      setActionError(attachmentErrorMessage(toCommandError(error).code));
      return;
    }
    if (!path) {
      return;
    }
    setBusy("add");
    try {
      const result = await addAttachment(documentId, path);
      onHistory(result.history);
      onChanged();
    } catch (error) {
      setActionError(attachmentErrorMessage(toCommandError(error).code));
    } finally {
      setBusy(null);
    }
  }

  async function extractTo(attachment: AttachmentView) {
    if (busy) {
      return;
    }
    setActionError(null);
    setNotice(null);
    let destination: string | null;
    try {
      destination = await chooseAttachmentDestination(attachment.name);
    } catch (error) {
      setActionError(attachmentErrorMessage(toCommandError(error).code));
      return;
    }
    if (!destination) {
      return;
    }
    setBusy("save");
    try {
      const result = await saveAttachmentAs(
        documentId,
        attachment.id,
        destination,
      );
      setNotice(`已另存到 ${result.path}`);
    } catch (error) {
      setActionError(attachmentErrorMessage(toCommandError(error).code));
    } finally {
      setBusy(null);
    }
  }

  async function runConfirmed() {
    const pending = confirmation;
    if (!pending || busy) {
      return;
    }
    setBusy(pending.kind);
    setActionError(null);
    setNotice(null);
    try {
      if (pending.kind === "open") {
        await openAttachment(documentId, pending.attachment.id, true);
      } else {
        const result = await deleteAttachment(
          documentId,
          pending.attachment.id,
        );
        onHistory(result.history);
        onChanged();
      }
    } catch (error) {
      setActionError(attachmentErrorMessage(toCommandError(error).code));
    } finally {
      setBusy(null);
      setConfirmation(null);
    }
  }

  return (
    <aside className="attachment-panel" aria-label="附件面板">
      <header className="attachment-panel-header">
        <h2>附件</h2>
        <span className="attachment-count">共 {attachments.length} 个</span>
      </header>

      {(error || actionError) && (
        <p className="attachment-error" role="alert">
          {actionError ?? error}
        </p>
      )}

      {notice && (
        <p className="attachment-notice" role="status">
          {notice}
        </p>
      )}

      {!canModify && (
        <p className="attachment-readonly">
          {readOnly
            ? "文档为只读，无法添加或删除附件。"
            : "当前文档的权限不允许添加或删除附件。"}
        </p>
      )}

      {loading && attachments.length === 0 ? (
        <p className="attachment-loading" role="status">
          正在载入附件…
        </p>
      ) : attachments.length === 0 ? (
        <p className="attachment-empty">还没有附件</p>
      ) : (
        <>
          <div className="attachment-columns" aria-hidden="true">
            <span>名称</span>
            <span>大小</span>
            <span>类型</span>
          </div>
          <ul className="attachment-items" aria-label="附件列表">
            {attachments.map((attachment) => {
              const disabled = busy !== null;
              return (
                <li key={attachment.id}>
                  <div className="attachment-meta">
                    <span className="attachment-name" title={attachment.name}>
                      {attachment.name}
                    </span>
                    <span className="attachment-size">
                      {formatAttachmentSize(attachment.size)}
                    </span>
                    <span className="attachment-type">
                      {attachment.mediaType}
                    </span>
                  </div>
                  <div className="attachment-actions">
                    <button
                      type="button"
                      className="secondary-button"
                      aria-label={`打开 ${attachment.name}`}
                      disabled={disabled}
                      onClick={() =>
                        setConfirmation({ kind: "open", attachment })
                      }
                    >
                      打开
                    </button>
                    <button
                      type="button"
                      className="secondary-button"
                      aria-label={`另存为 ${attachment.name}`}
                      disabled={disabled}
                      onClick={() => void extractTo(attachment)}
                    >
                      另存为
                    </button>
                    <button
                      type="button"
                      className="danger-button"
                      aria-label={`删除 ${attachment.name}`}
                      disabled={!canModify || disabled}
                      onClick={() =>
                        setConfirmation({ kind: "delete", attachment })
                      }
                    >
                      删除
                    </button>
                  </div>
                </li>
              );
            })}
          </ul>
        </>
      )}

      <button
        type="button"
        className="primary-button attachment-add"
        disabled={!canModify || busy !== null}
        onClick={() => void addFile()}
      >
        添加附件
      </button>

      {confirmation && (
        <div className="password-overlay">
          <div
            className="password-dialog attachment-confirm"
            role="dialog"
            aria-modal="true"
            aria-labelledby="attachment-confirm-title"
          >
            <h2 id="attachment-confirm-title">
              {confirmation.kind === "open" ? "打开附件？" : "删除附件？"}
            </h2>
            {confirmation.kind === "open" ? (
              <>
                <p className="password-hint">{ATTACHMENT_OPEN_WARNING}</p>
                <p className="password-hint">
                  文件会先解压到临时目录，再用系统默认程序打开。
                </p>
              </>
            ) : (
              <p className="password-hint">
                “{confirmation.attachment.name}”将在保存文档后从 PDF
                中移除。你可以使用撤销恢复。
              </p>
            )}
            <div className="password-actions">
              <button
                type="button"
                className="secondary-button"
                disabled={busy !== null}
                onClick={() => setConfirmation(null)}
              >
                取消
              </button>
              <button
                type="button"
                className={
                  confirmation.kind === "open"
                    ? "open-button"
                    : "danger-button"
                }
                disabled={busy !== null}
                onClick={() => void runConfirmed()}
              >
                {confirmation.kind === "open" ? "打开" : "删除"}
              </button>
            </div>
          </div>
        </div>
      )}
    </aside>
  );
}
