import { WorkspaceIcon } from "./WorkspaceIcon";
import type { CommandErrorCode, DocumentView, SaveProgress } from "../lib/backend";
import {
  documentPolicy,
  READ_ONLY_REASON,
  SIGNATURE_SAVE_CONFIRM_REQUIRED,
} from "../lib/document-policy";
import type { DocumentSave } from "../lib/useDocumentSave";

const SAVE_ERROR_MESSAGES: Partial<Record<CommandErrorCode, string>> = {
  save_failed: "保存失败：无法写入目标文件。请检查磁盘空间和权限后重试。",
  save_target_is_source: "不能把源文件本身作为输出，请选择其他位置。",
  no_output_document: "此文档还没有输出文件，请先另存为。",
  unencrypted_output_confirmation_required:
    "保存加密文档前需要确认输出不加密。",
  encrypted_output_confirmation_required:
    "当前输出文件已加密；请重新设置打开密码，或明确选择不加密保存。",
  signature_invalidation_confirmation_required: SIGNATURE_SAVE_CONFIRM_REQUIRED,
  invalid_argument: "保存设置无效，请检查后重试。",
  read_only_document: "只读文档不能保存更改。",
  permission_denied: "没有写入目标位置的权限。",
  file_read_failed: "无法读取源文件。",
  internal: "保存时发生内部错误。",
};

function saveErrorMessage(code: CommandErrorCode): string {
  return SAVE_ERROR_MESSAGES[code] ?? "保存失败，请重试。";
}

function progressText(progress: SaveProgress | null): string {
  if (!progress) {
    return "正在保存…";
  }
  switch (progress.stage) {
    case "preparing":
      return "正在准备保存…";
    case "serializing":
      return "正在生成 PDF…";
    case "finalizing":
      return "正在应用对象级更改…";
    case "writing":
      return "正在写入文件…";
    case "done":
      return "保存完成";
  }
}

function baseName(path: string): string {
  const segments = path.split(/[\\/]/);
  return segments[segments.length - 1] || path;
}

/**
 * Primary save entries in the document header.
 *
 * Save Changes is disabled for a Permission-Restricted Document while Save As
 * stays available so the user can keep a copy; an encrypted document carries
 * its unencrypted-output warning in the notice strip (ADR-0007).
 */
export function DocumentSaveControls({
  document,
  save,
}: {
  document: DocumentView;
  save: DocumentSave;
}) {
  const policy = documentPolicy(document);

  return (
    <div className="document-save-controls" role="group" aria-label="保存">
      <button
        type="button"
        className="header-button header-button--primary"
        disabled={!save.canSaveChanges || save.busy}
        aria-describedby={document.readOnly ? "read-only-reason" : undefined}
        title={policy.saveChangesDisabledReason ?? "保存到当前输出文件"}
        onClick={save.saveChanges}
      >
        <span>保存更改</span><WorkspaceIcon name="save" />
      </button>
      <button
        type="button"
        className="header-button"
        disabled={save.busy}
        title={policy.unencryptedOutputWarning ?? "选择输出文件并保存"}
        onClick={save.saveAs}
      >
        另存为…
      </button>
    </div>
  );
}

/**
 * Document-level notices that do not belong next to the save buttons:
 * read-only policy, encryption warnings, save progress, the last written path
 * and retryable failures. Rendered as a slim strip above the workspace so the
 * editing chrome stays quiet.
 */
export function DocumentNotices({
  document,
  save,
}: {
  document: DocumentView;
  save: DocumentSave;
}) {
  const policy = documentPolicy(document);
  const retryLabel =
    save.error?.error.code === "save_target_is_source" ||
    save.error?.error.code === "no_output_document"
      ? "重新选择…"
      : "重试保存";

  const hasNotice =
    document.readOnly ||
    save.busy ||
    (!save.busy && save.lastSavedPath) ||
    save.error !== null ||
    policy.unencryptedOutputWarning !== null;
  if (!hasNotice) {
    return null;
  }

  return (
    <div className="document-notices">
      {document.readOnly && (
        <p className="document-notice" id="read-only-reason">
          {READ_ONLY_REASON}
        </p>
      )}
      {policy.unencryptedOutputWarning && (
        <p
          className="document-notice document-notice--warning"
          id="unencrypted-output-warning"
        >
          {policy.unencryptedOutputWarning}
        </p>
      )}
      {save.busy && (
        <p className="document-notice" aria-live="polite">
          {progressText(save.progress)}
        </p>
      )}
      {!save.busy && save.lastSavedPath && (
        <p className="document-notice">
          已保存到{" "}
          <span title={save.lastSavedPath}>{baseName(save.lastSavedPath)}</span>
        </p>
      )}
      {save.error && (
        <div className="document-notice document-notice--error" role="alert">
          <p>{saveErrorMessage(save.error.error.code)}</p>
          <p className="error-detail">{save.error.error.message}</p>
          <div className="save-error-actions">
            <button type="button" onClick={save.retry}>
              {retryLabel}
            </button>
            <button type="button" onClick={save.dismissError}>
              关闭
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
