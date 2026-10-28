import { useEffect, useMemo, useState } from "react";

import {
  deleteAnnotation,
  toCommandError,
  updateAnnotationStyle,
  updateShapeStyle,
  type AnnotationKind,
  type AnnotationView,
  type ShapeStyle,
} from "../lib/backend";
import { notifyDocumentChanged } from "../lib/documentEvents";

const KIND_LABELS: Record<AnnotationKind, string> = {
  highlight: "高亮",
  underline: "下划线",
  strikeout: "删除线",
  square: "矩形",
  circle: "椭圆",
};

const DEFAULT_SHAPE_FILL = "#3366cc";
const DEFAULT_SHAPE_STROKE = "#000000";
const SHAPE_LINE_WIDTH_MIN = 0.5;
const SHAPE_LINE_WIDTH_MAX = 100;

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
  const [shapeFillEnabled, setShapeFillEnabled] = useState(
    selected?.shape?.style.fillColor !== null,
  );
  const [shapeFillColor, setShapeFillColor] = useState(
    selected?.shape?.style.fillColor ?? DEFAULT_SHAPE_FILL,
  );
  const [shapeStrokeEnabled, setShapeStrokeEnabled] = useState(
    selected?.shape?.style.strokeColor !== null,
  );
  const [shapeStrokeColor, setShapeStrokeColor] = useState(
    selected?.shape?.style.strokeColor ?? DEFAULT_SHAPE_STROKE,
  );
  const [shapeLineWidth, setShapeLineWidth] = useState(
    selected?.shape?.style.lineWidth ?? 2,
  );
  const [busy, setBusy] = useState<"style" | "delete" | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const shapeStyle = selected?.shape?.style ?? null;

  useEffect(() => {
    setColorDraft(selected?.color ?? "#ffd400");
    setOpacityDraft(selected?.opacity ?? 50);
    setShapeFillEnabled(shapeStyle ? shapeStyle.fillColor !== null : true);
    setShapeFillColor(shapeStyle?.fillColor ?? DEFAULT_SHAPE_FILL);
    setShapeStrokeEnabled(shapeStyle ? shapeStyle.strokeColor !== null : true);
    setShapeStrokeColor(shapeStyle?.strokeColor ?? DEFAULT_SHAPE_STROKE);
    setShapeLineWidth(shapeStyle?.lineWidth ?? 2);
    setActionError(null);
  }, [
    selected?.name,
    selected?.color,
    selected?.opacity,
    shapeStyle?.fillColor,
    shapeStyle?.strokeColor,
    shapeStyle?.lineWidth,
  ]);

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
    selected.shape === null &&
    (colorDraft !== selected.color || opacityDraft !== selected.opacity);
  const shapeDraft: ShapeStyle | null = shapeStyle
    ? {
        fillColor: shapeFillEnabled ? shapeFillColor : null,
        strokeColor: shapeStrokeEnabled ? shapeStrokeColor : null,
        opacity: opacityDraft,
        lineWidth: shapeLineWidth,
      }
    : null;
  const shapeDraftValid = shapeFillEnabled || shapeStrokeEnabled;
  const shapeStyleChanged =
    shapeStyle !== null &&
    shapeDraft !== null &&
    (shapeDraft.fillColor !== shapeStyle.fillColor ||
      shapeDraft.strokeColor !== shapeStyle.strokeColor ||
      shapeDraft.opacity !== shapeStyle.opacity ||
      shapeDraft.lineWidth !== shapeStyle.lineWidth);
  const canApply =
    selected !== null &&
    selected.shape === null
      ? styleChanged
      : shapeStyleChanged && shapeDraftValid;

  async function applyStyle() {
    if (!selected || !canApply || busy) {
      return;
    }
    setBusy("style");
    setActionError(null);
    try {
      if (shapeDraft) {
        await updateShapeStyle(documentId, selected.name, shapeDraft);
      } else {
        await updateAnnotationStyle(
          documentId,
          selected.name,
          colorDraft,
          opacityDraft,
        );
      }
      onChanged();
      notifyDocumentChanged({
        documentId,
        pageIndex: selected.pageIndex,
        source: "annotation",
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
        source: "annotation",
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
          <p>
            选择页面文字即可添加高亮、下划线或删除线；使用页面上的形状工具绘制矩形或椭圆。
          </p>
          <p>直线与箭头是页面内容，会直接画在页面上，不会出现在此列表。</p>
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
          {selected.shape ? (
            <>
              <p className="annotation-shape-meta">
                {KIND_LABELS[selected.kind]} · 尺寸{" "}
                {Math.round(selected.shape.geometry.width)} ×{" "}
                {Math.round(selected.shape.geometry.height)} pt · 旋转{" "}
                {Math.round(selected.shape.geometry.rotation)}°
              </p>
              <label className="annotation-control">
                <span>填充</span>
                <input
                  type="checkbox"
                  aria-label="形状填充"
                  checked={shapeFillEnabled}
                  disabled={busy !== null}
                  onChange={(event) =>
                    setShapeFillEnabled(event.target.checked)
                  }
                />
                <input
                  type="color"
                  aria-label="形状填充颜色"
                  value={shapeFillColor}
                  disabled={!shapeFillEnabled || busy !== null}
                  onChange={(event) => setShapeFillColor(event.target.value)}
                />
              </label>
              <label className="annotation-control">
                <span>描边</span>
                <input
                  type="checkbox"
                  aria-label="形状描边"
                  checked={shapeStrokeEnabled}
                  disabled={busy !== null}
                  onChange={(event) =>
                    setShapeStrokeEnabled(event.target.checked)
                  }
                />
                <input
                  type="color"
                  aria-label="形状描边颜色"
                  value={shapeStrokeColor}
                  disabled={!shapeStrokeEnabled || busy !== null}
                  onChange={(event) => setShapeStrokeColor(event.target.value)}
                />
              </label>
              <label className="annotation-control">
                <span>线宽</span>
                <input
                  type="number"
                  aria-label="形状线宽"
                  min={SHAPE_LINE_WIDTH_MIN}
                  max={SHAPE_LINE_WIDTH_MAX}
                  step={0.5}
                  value={shapeLineWidth}
                  disabled={!shapeStrokeEnabled || busy !== null}
                  onChange={(event) => {
                    const next = Number.parseFloat(event.target.value);
                    if (Number.isFinite(next)) {
                      setShapeLineWidth(
                        Math.min(
                          Math.max(next, SHAPE_LINE_WIDTH_MIN),
                          SHAPE_LINE_WIDTH_MAX,
                        ),
                      );
                    }
                  }}
                />
              </label>
              <label className="annotation-control">
                <span>透明度</span>
                <input
                  type="range"
                  aria-label="形状透明度"
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
              {!shapeDraftValid && (
                <p className="annotation-shape-warning" role="status">
                  至少需要填充或描边中的一项
                </p>
              )}
            </>
          ) : (
            <>
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
            </>
          )}
          <div className="annotation-actions">
            <button
              type="button"
              disabled={!canApply || busy !== null}
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
