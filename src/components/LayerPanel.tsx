import { useCallback, useEffect, useRef, useState } from "react";

import {
  deleteLayer,
  listLayers,
  renameLayer,
  setLayerVisible,
  toCommandError,
  type CommandErrorCode,
  type EditHistoryView,
  type LayerView,
} from "../lib/backend";
import { onDocumentChanged } from "../lib/documentEvents";

/** The backend's cap for a layer name, counted in Unicode code points. */
export const MAX_LAYER_NAME_CHARS = 120;

const LAYER_ERROR_MESSAGES: Partial<Record<CommandErrorCode, string>> = {
  permission_denied: "当前文档的权限不允许修改图层。",
  read_only_document: "文档为只读，无法修改图层。",
  document_not_found: "文档已关闭。",
  layer_not_found: "该图层已不在文档中。",
  layer_name_invalid:
    "图层名称无效：不能为空、不能包含控制字符，且不能超过 120 个字符。",
  layer_delete_unsupported:
    "该图层包含暂不支持安全删除的内容，未做任何更改。",
  invalid_argument: "图层操作无效，请重试。",
  render_failed: "无法重新渲染页面。",
};

/** Chinese copy for a layer command failure. */
export function layerErrorMessage(code: CommandErrorCode): string {
  return LAYER_ERROR_MESSAGES[code] ?? "图层操作失败，请重试。";
}

/** Validates a name the same way the Rust command does. */
function layerNameError(value: string): string | null {
  const name = value.trim();
  if (!name) {
    return "图层名称不能为空。";
  }
  if (/[\u0000-\u001f\u007f-\u009f]/.test(name)) {
    return "图层名称不能包含控制字符。";
  }
  if (Array.from(name).length > MAX_LAYER_NAME_CHARS) {
    return `图层名称不能超过 ${MAX_LAYER_NAME_CHARS} 个字符。`;
  }
  return null;
}

interface RenameState {
  index: number;
  value: string;
}

export interface LayerPanelProps {
  documentId: string;
  /** Permission Restrictions allow changing page content. */
  canModifyContent: boolean;
  /** Permission Restrictions deny every editing capability. */
  readOnly: boolean;
  /** Feeds the unified history so the dirty marker and menu update. */
  onHistory: (history: EditHistoryView) => void;
  /**
   * Asks the owner to repaint the pages after a visibility change or a
   * deletion. A rename changes no pixels, so it passes false.
   */
  onChanged: (repaint: boolean) => void;
}

/**
 * Document-wide optional-content layer list.
 *
 * The PDF's `/OCProperties` are managed by the Rust layer store, so the panel
 * is an ordinary view over it: toggling and deleting repaint every page through
 * the shadow PDFium document, renaming only changes metadata, and all three are
 * undoable actions in the unified history. The backend refuses deletions it
 * cannot perform safely; that refusal is shown without changing the list.
 */
export function LayerPanel({
  documentId,
  canModifyContent,
  readOnly,
  onHistory,
  onChanged,
}: LayerPanelProps) {
  const editable = canModifyContent && !readOnly;
  const [layers, setLayers] = useState<LayerView[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [busy, setBusy] = useState<"toggle" | "rename" | "delete" | null>(
    null,
  );
  const [renaming, setRenaming] = useState<RenameState | null>(null);
  const [confirmation, setConfirmation] = useState<LayerView | null>(null);
  const generationRef = useRef(0);

  const load = useCallback(async () => {
    const request = ++generationRef.current;
    try {
      const view = await listLayers(documentId);
      if (generationRef.current !== request) {
        return;
      }
      if (view.documentId === documentId) {
        setLayers(view.layers);
      }
      setError(null);
      setLoading(false);
    } catch (failure) {
      if (generationRef.current !== request) {
        return;
      }
      setError(layerErrorMessage(toCommandError(failure).code));
      setLoading(false);
    }
  }, [documentId]);

  // A new document starts from an empty list; the old response cannot leak
  // into the new panel because `load` invalidates the previous generation.
  useEffect(() => {
    generationRef.current += 1;
    setLayers([]);
    setLoading(true);
    setError(null);
    setActionError(null);
    setRenaming(null);
    setConfirmation(null);
    void load();
    return () => {
      generationRef.current += 1;
    };
  }, [documentId, load]);

  // Undo and redo arrive as all-pages changes; our own commands also announce
  // a layer change. Refetch so a history step restores names and visibility.
  useEffect(
    () =>
      onDocumentChanged((detail) => {
        if (
          detail.documentId === documentId &&
          (detail.allPagesChanged ||
            detail.pageStructureChanged ||
            detail.source === "layer")
        ) {
          void load();
        }
      }),
    [documentId, load],
  );

  // A command result invalidates an in-flight list refresh: its response may
  // predate this action and would overwrite the newer list.
  async function toggleVisibility(layer: LayerView, visible: boolean) {
    if (!editable || busy) {
      return;
    }
    setActionError(null);
    setBusy("toggle");
    try {
      const result = await setLayerVisible(documentId, layer.index, visible);
      generationRef.current += 1;
      setLayers(result.layers);
      onHistory(result.history);
      onChanged(true);
    } catch (failure) {
      setActionError(layerErrorMessage(toCommandError(failure).code));
    } finally {
      setBusy(null);
    }
  }

  function startRename(layer: LayerView) {
    if (!editable || busy) {
      return;
    }
    setActionError(null);
    setRenaming({ index: layer.index, value: layer.name });
  }

  async function submitRename(layer: LayerView) {
    if (!editable || busy || !renaming || renaming.index !== layer.index) {
      return;
    }
    const message = layerNameError(renaming.value);
    if (message) {
      setActionError(message);
      return;
    }
    setActionError(null);
    setBusy("rename");
    try {
      const result = await renameLayer(
        documentId,
        layer.index,
        renaming.value.trim(),
      );
      generationRef.current += 1;
      setLayers(result.layers);
      setRenaming(null);
      onHistory(result.history);
      // A name is metadata: no page pixels or thumbnails are stale.
      onChanged(false);
    } catch (failure) {
      setActionError(layerErrorMessage(toCommandError(failure).code));
    } finally {
      setBusy(null);
    }
  }

  async function confirmDelete() {
    const target = confirmation;
    if (!target || busy) {
      return;
    }
    setBusy("delete");
    setActionError(null);
    try {
      const result = await deleteLayer(documentId, target.index);
      generationRef.current += 1;
      setLayers(result.layers);
      onHistory(result.history);
      onChanged(true);
    } catch (failure) {
      setActionError(layerErrorMessage(toCommandError(failure).code));
    } finally {
      setBusy(null);
      setConfirmation(null);
    }
  }

  const disabled = !editable || busy !== null;
  const shownError = actionError ?? error;

  return (
    <aside className="layer-panel" aria-label="图层面板">
      <header className="layer-panel-header">
        <h2>图层</h2>
        <span className="layer-count">共 {layers.length} 个</span>
      </header>

      {!editable && (
        <p className="layer-readonly" role="note">
          {readOnly
            ? "文档为只读，无法修改图层。"
            : "当前文档的权限不允许修改图层。"}
        </p>
      )}

      {shownError && (
        <p className="layer-error" role="alert">
          {shownError}
        </p>
      )}

      {loading ? (
        <p className="layer-loading" role="status">
          正在载入图层…
        </p>
      ) : layers.length === 0 ? (
        error ? null : (
          <p className="layer-empty">此文档没有可编辑图层</p>
        )
      ) : (
        <ul className="layer-items" aria-label="图层列表">
          {layers.map((layer) => (
            <li key={layer.index} className="layer-item">
              {renaming?.index === layer.index ? (
                <form
                  className="layer-rename"
                  onSubmit={(event) => {
                    event.preventDefault();
                    void submitRename(layer);
                  }}
                >
                  <input
                    type="text"
                    className="layer-rename-input"
                    aria-label={`图层名称 ${layer.name}`}
                    value={renaming.value}
                    maxLength={MAX_LAYER_NAME_CHARS}
                    autoFocus
                    disabled={busy !== null}
                    onChange={(event) =>
                      setRenaming({
                        index: layer.index,
                        value: event.target.value,
                      })
                    }
                    onKeyDown={(event) => {
                      if (event.key === "Escape") {
                        setRenaming(null);
                        setActionError(null);
                      }
                    }}
                  />
                  <div className="layer-rename-actions">
                    <button
                      type="submit"
                      className="secondary-button"
                      disabled={busy !== null}
                    >
                      保存
                    </button>
                    <button
                      type="button"
                      className="secondary-button"
                      disabled={busy !== null}
                      onClick={() => {
                        setRenaming(null);
                        setActionError(null);
                      }}
                    >
                      取消
                    </button>
                  </div>
                </form>
              ) : (
                <>
                  <label className="layer-visibility">
                    <input
                      type="checkbox"
                      checked={layer.visible}
                      disabled={disabled}
                      aria-label={`显示 ${layer.name}`}
                      onChange={(event) =>
                        void toggleVisibility(layer, event.target.checked)
                      }
                    />
                    <span className="layer-name" title={layer.name}>
                      {layer.name}
                    </span>
                  </label>
                  <div className="layer-actions">
                    <button
                      type="button"
                      className="secondary-button"
                      disabled={disabled}
                      aria-label={`重命名 ${layer.name}`}
                      onClick={() => startRename(layer)}
                    >
                      重命名
                    </button>
                    <button
                      type="button"
                      className="danger-button"
                      disabled={disabled}
                      aria-label={`删除 ${layer.name}`}
                      onClick={() => {
                        setActionError(null);
                        setConfirmation(layer);
                      }}
                    >
                      删除
                    </button>
                  </div>
                </>
              )}
            </li>
          ))}
        </ul>
      )}

      {confirmation && (
        <div className="password-overlay">
          <div
            className="password-dialog layer-confirm"
            role="dialog"
            aria-modal="true"
            aria-labelledby="layer-delete-title"
          >
            <h2 id="layer-delete-title">删除图层？</h2>
            <p className="password-hint">
              “{confirmation.name}”及其内容将在保存文档后从 PDF
              中移除。删除会同时移除该图层上的文字、图片等内容。
            </p>
            <p className="password-hint">
              在关闭文档前，你可以使用撤销恢复这个图层；关闭后无法撤销。
            </p>
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
                className="danger-button"
                disabled={busy !== null}
                onClick={() => void confirmDelete()}
              >
                {busy === "delete" ? "正在删除…" : "删除"}
              </button>
            </div>
          </div>
        </div>
      )}
    </aside>
  );
}
