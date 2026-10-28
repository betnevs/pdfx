import { useEffect, useState } from "react";

import {
  chooseDiagnosticsDestination,
  exportDiagnostics,
  type AssociationResult,
  type InterfaceLanguage,
  type Settings,
  type SettingsUpdateResult,
  type Theme,
} from "../lib/settings";

interface SettingsPanelProps {
  open: boolean;
  settings: Settings;
  onClose: () => void;
  onSave: (next: Settings) => Promise<SettingsUpdateResult>;
}

type SaveStatus =
  | { kind: "idle" }
  | { kind: "saving" }
  | { kind: "saved" }
  | { kind: "error"; message: string };

type ExportStatus =
  | { kind: "idle" }
  | { kind: "exporting" }
  | { kind: "done"; path: string }
  | { kind: "error"; message: string };

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function saveStatusText(status: SaveStatus): string {
  switch (status.kind) {
    case "saving":
      return "正在保存…";
    case "saved":
      return "已保存";
    case "idle":
    case "error":
      return "";
  }
}

function associationMessage(result: AssociationResult): string {
  if (result.status === "failed") {
    return result.message ?? "文件关联操作失败。";
  }
  return result.instructions;
}

/**
 * The settings page.
 *
 * Every control commits as soon as it changes, so settings take effect without
 * a restart while remaining persisted by the backend. The panel deliberately
 * contains no telemetry or automatic-upload options.
 */
export function SettingsPanel({ open, settings, onClose, onSave }: SettingsPanelProps) {
  const [draft, setDraft] = useState<Settings>(settings);
  const [saveStatus, setSaveStatus] = useState<SaveStatus>({ kind: "idle" });
  const [association, setAssociation] = useState<AssociationResult | null>(null);
  const [exportStatus, setExportStatus] = useState<ExportStatus>({ kind: "idle" });

  useEffect(() => {
    if (open) {
      setDraft(settings);
    }
  }, [open, settings]);

  useEffect(() => {
    if (!open) {
      return;
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onClose();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [open, onClose]);

  if (!open) {
    return null;
  }

  /**
   * Builds the next settings from the draft. An empty author field falls back
   * to the last persisted name so unrelated controls keep working while the
   * field is being edited.
   */
  function nextSettings(overrides: Partial<Settings>): Settings {
    const author = draft.annotationAuthor.trim();
    return {
      ...draft,
      annotationAuthor: author.length > 0 ? author : settings.annotationAuthor,
      ...overrides,
    };
  }

  async function commit(next: Settings) {
    setDraft(next);
    setSaveStatus({ kind: "saving" });
    try {
      const result = await onSave(next);
      setDraft(result.settings);
      setAssociation(result.association);
      setSaveStatus({ kind: "saved" });
    } catch (error) {
      setDraft(settings);
      setAssociation(null);
      setSaveStatus({
        kind: "error",
        message: `保存失败：${describeError(error)}`,
      });
    }
  }

  function commitAuthor() {
    const author = draft.annotationAuthor.trim();
    if (author.length === 0) {
      setSaveStatus({
        kind: "error",
        message: "Annotation 显示名不能为空。",
      });
      return;
    }
    if (author === settings.annotationAuthor) {
      setDraft({ ...draft, annotationAuthor: author });
      if (saveStatus.kind === "error") {
        setSaveStatus({ kind: "idle" });
      }
      return;
    }
    void commit(nextSettings({ annotationAuthor: author }));
  }

  async function exportNow() {
    setExportStatus({ kind: "exporting" });
    try {
      const destination = await chooseDiagnosticsDestination();
      if (!destination) {
        setExportStatus({ kind: "idle" });
        return;
      }
      const exported = await exportDiagnostics(destination);
      setExportStatus({ kind: "done", path: exported.path });
    } catch (error) {
      setExportStatus({
        kind: "error",
        message: `导出失败：${describeError(error)}`,
      });
    }
  }

  return (
    <div
      className="settings-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) {
          onClose();
        }
      }}
    >
      <section
        className="settings-panel"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
      >
        <header className="settings-header">
          <h2 id="settings-title">设置</h2>
          <button
            type="button"
            className="settings-close"
            aria-label="关闭设置"
            onClick={onClose}
          >
            ×
          </button>
        </header>

        <div className="settings-body">
          <section className="settings-section">
            <h3>外观与语言</h3>
            <label className="settings-field">
              <span>界面语言</span>
              <select
                value={draft.language}
                onChange={(event) =>
                  void commit(nextSettings({ language: event.target.value as InterfaceLanguage }))
                }
              >
                <option value="zh-CN">简体中文</option>
                <option value="en-US">English</option>
              </select>
            </label>
            <label className="settings-field">
              <span>主题</span>
              <select
                value={draft.theme}
                onChange={(event) =>
                  void commit(nextSettings({ theme: event.target.value as Theme }))
                }
              >
                <option value="system">跟随系统</option>
                <option value="light">浅色</option>
                <option value="dark">深色</option>
              </select>
            </label>
          </section>

          <section className="settings-section">
            <h3>Annotation</h3>
            <label className="settings-field">
              <span>Annotation 显示名</span>
              <input
                type="text"
                maxLength={64}
                value={draft.annotationAuthor}
                onChange={(event) =>
                  setDraft({ ...draft, annotationAuthor: event.target.value })
                }
                onBlur={commitAuthor}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.currentTarget.blur();
                  }
                }}
              />
            </label>
            <p className="settings-hint">新建 Annotation 会使用此名称作为作者。</p>
          </section>

          <section className="settings-section">
            <h3>文件关联</h3>
            <label className="settings-checkbox">
              <input
                type="checkbox"
                checked={draft.fileAssociationEnabled}
                onChange={(event) =>
                  void commit(
                    nextSettings({ fileAssociationEnabled: event.target.checked }),
                  )
                }
              />
              <span>将 PDFX 注册为 PDF 打开方式</span>
            </label>
            <p className="settings-hint">
              默认关闭。现代 macOS 与 Windows 不允许应用静默接管默认 PDF
              打开方式；开启后，请在系统设置中确认 PDFX。
            </p>
            {association && (
              <p
                className="settings-guidance"
                role={association.status === "failed" ? "alert" : "note"}
              >
                {associationMessage(association)}
              </p>
            )}
          </section>

          <section className="settings-section">
            <h3>更新检查</h3>
            <p className="settings-hint">
              PDFX 启动和后台都不会联网。更新检查只会在你主动点击时进行，不会自动下载或安装。
            </p>
            <button type="button" className="secondary-button" disabled>
              检查更新
            </button>
          </section>

          <section className="settings-section">
            <h3>诊断</h3>
            <p className="settings-hint">
              本地滚动日志只保存在这台设备上。诊断包由你选择保存位置，包含日志、版本与平台信息，不会自动上传。
            </p>
            <button
              type="button"
              className="secondary-button"
              disabled={exportStatus.kind === "exporting"}
              onClick={() => void exportNow()}
            >
              导出诊断包…
            </button>
            {exportStatus.kind === "done" && (
              <p className="settings-hint" role="status">
                诊断包已保存到 {exportStatus.path}
              </p>
            )}
            {exportStatus.kind === "error" && (
              <p className="settings-guidance" role="alert">
                {exportStatus.message}
              </p>
            )}
          </section>
        </div>

        <footer className="settings-footer">
          <span role="status" aria-live="polite">
            {saveStatusText(saveStatus)}
          </span>
          {saveStatus.kind === "error" && <span role="alert">{saveStatus.message}</span>}
        </footer>
      </section>
    </div>
  );
}
