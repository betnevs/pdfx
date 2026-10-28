import { useEffect, useMemo, useState } from "react";

import {
  deleteAnnotation,
  toCommandError,
  updateAnnotationStyle,
  type AnnotationView,
  type TextMarkupKind,
} from "../lib/backend";
import { notifyDocumentChanged } from "../lib/documentEvents";

const KIND_LABELS: Record<TextMarkupKind, string> = {
  highlight: "高亮",
  underline: "下划线",
  strikeout: "删除线",
};

/** Formats the backend's PDF date into the list's readable UTC label. */
export function formatAnnotationTime(value: string | null): string | null {
  if (!value) {
    return null;
  }
  const match = /^D:(\d{4})(\d{2})(\d{2})(\d{2})(\d{2})(\d{2})Z?$/.exec(value);
  if (!match) {
    return value;
  }
  const [, year, month, day, hour, minute] = match;
  return `${year}-${month}-${day} ${hour}:${minute} UTC`;
}

export interface AnnotationPanelProps {
  documentId: string;
  annotations: AnnotationView[];
  selectedName: string | null;
  loading?: boolean;
  error?: string | null;
  /** PDF permission for adding and modifying annotations. */
  canAnnotate: boolean;
  /** Permission Restrictions deny every editing capability. */
  readOnly: boolean;
  onSelect: (annotation: AnnotationView | null) => void;
  /** Asks the owner to refetch after a mutation. */
  onChanged: () => void;
}

/**
 * Document-wide annotation list, grouped by page.
 *
 * Selecting an entry asks the workspace to navigate to its page and draws the
 * selection outline on the page. The selected entry exposes colour, opacity and
 * delete controls; every mutation goes through the unified history, so the
 * page's undo/redo buttons restore it.
 */
export function AnnotationPanel({
  documentId,
  annotations,
  selectedName,
  loading = false,
  error = null,
  canAnnotate,
  readOnly,
  onSelect,
  onChanged,
}: AnnotationPanelProps) {
  const selected =
    annotations.find((annotation) => annotation.name === selectedName) ?? null;
  const [colorDraft, setColorDraft] = useState(selected?.color ?? "#ffd400");
  const [opacityDraft, setOpacityDraft] = useState(selected?.opacity ?? 50);
  const [busy, setBusy] = useState<"style" | "delete" | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  useEffect(() => {
    setColorDraft(selected?.color ?? "#ffd400");
    setOpacityDraft(selected?.opacity ?? 50);
    setActionError(null);
  }, [selected?.name, selected?.color, selected?.opacity]);

  const groups = useMemo(() => {
    const byPage = new Map<number, AnnotationView[]>();
    for (const annotation of annotations) {
      const page = byPage.get(annotation.pageIndex);
      if (page) {
        page.push(annotation);
      } else {
        byPage.set(annotation.pageIndex, [annotation]);
      }
    }
    return [...byPage.entries()].sort(([left], [right]) => left - right);
  }, [annotations]);

  const editable = canAnnotate && !readOnly;
  const styleChanged =
    selected !== null &&
    (colorDraft !== selected.color || opacityDraft !== selected.opacity);

  async function applyStyle() {
    if (!selected || !styleChanged || busy) {
      return;
    }
    setBusy("style");
    setActionError(null);
    try {
      await updateAnnotationStyle(
        documentId,
        selected.name,
        colorDraft,
        opacityDraft,
      );
      onChanged();
      notifyDocumentChanged({
        documentId,
        pageIndex: selected.pageIndex,
      });
    } catch (error) {
      setActionError(toCommandError(error).message);
    } finally {
      setBusy(null);
    }
  }

  async function removeSelected() {
    if (!selected || busy) {
      return;
    }
    setBusy("delete");
    setActionError(null);
    try {
      await deleteAnnotation(documentId, selected.name);
      onSelect(null);
      onChanged();
      notifyDocumentChanged({
        documentId,
        pageIndex: selected.pageIndex,
      });
    } catch (error) {
      setActionError(toCommandError(error).message);
    } finally {
      setBusy(null);
    }
  }

  return (
    <aside className="annotation-panel" aria-label="注释列表">
      <header className="annotation-panel-header">
        <h2>注释</h2>
        <span className="annotation-count">共 {annotations.length} 条</span>
      </header>

      {(error || actionError) && (
        <p className="annotation-error" role="alert">
          {actionError ?? error}
        </p>
      )}

      {!editable && (
        <p className="annotation-readonly">此文档不允许添加或修改注释</p>
      )}

      {loading && annotations.length === 0 ? (
        <p className="annotation-loading" role="status">
          正在载入注释…
        </p>
      ) : annotations.length === 0 ? (
        <div className="annotation-empty">
          <p>还没有注释</p>
          <p>选择页面文字即可添加高亮、下划线或删除线。</p>
        </div>
      ) : (
        <div className="annotation-groups">
          {groups.map(([pageIndex, pageAnnotations]) => (
            <section
              key={pageIndex}
              className="annotation-group"
              aria-label={`第 ${pageIndex + 1} 页的注释`}
            >
              <div className="annotation-group-heading">
                <h3>第 {pageIndex + 1} 页</h3>
                <span>{pageAnnotations.length} 条</span>
              </div>
              <ul className="annotation-items">
                {pageAnnotations.map((annotation) => {
                  const label = KIND_LABELS[annotation.kind];
                  const created = formatAnnotationTime(annotation.createdAt);
                  return (
                    <li key={annotation.name}>
                      <button
                        type="button"
                        className="annotation-item"
                        aria-label={`${label}，第 ${pageIndex + 1} 页，${annotation.author ?? "未知作者"}`}
                        aria-pressed={annotation.name === selectedName}
                        onClick={() => onSelect(annotation)}
                      >
                        <span
                          className="annotation-swatch"
                          aria-hidden="true"
                          style={{
                            backgroundColor: annotation.color,
                            opacity: annotation.opacity / 100,
                          }}
                        />
                        <span className="annotation-item-body">
                          <span className="annotation-kind">{label}</span>
                          <span className="annotation-meta">
                            {annotation.author ?? "未知作者"}
                          </span>
                          <span className="annotation-time">
                            {created ?? "时间未知"}
                          </span>
                        </span>
                      </button>
                    </li>
                  );
                })}
              </ul>
            </section>
          ))}
        </div>
      )}

      {selected && editable && (
        <div className="annotation-properties" aria-label="注释属性">
          <label className="annotation-control">
            <span>颜色</span>
            <input
              type="color"
              aria-label="注释颜色"
              value={colorDraft}
              disabled={busy !== null}
              onChange={(event) => setColorDraft(event.target.value)}
            />
          </label>
          <label className="annotation-control">
            <span>透明度</span>
            <input
              type="range"
              aria-label="注释透明度"
              min={0}
              max={100}
              step={1}
              value={opacityDraft}
              disabled={busy !== null}
              onChange={(event) =>
                setOpacityDraft(Number.parseInt(event.target.value, 10))
              }
            />
            <span>{opacityDraft}%</span>
          </label>
          <div className="annotation-actions">
            <button
              type="button"
              disabled={!styleChanged || busy !== null}
              onClick={() => void applyStyle()}
            >
              应用样式
            </button>
            <button
              type="button"
              className="annotation-delete"
              disabled={busy !== null}
              onClick={() => void removeSelected()}
            >
              删除注释
            </button>
          </div>
        </div>
      )}
    </aside>
  );
}
