import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
} from "react";

import {
  createFreeTextAnnotation,
  toCommandError,
  transformFreeTextAnnotation,
  updateFreeText,
  type AnnotationView,
  type FreeTextAlign,
  type FreeTextKind,
  type TextMarkupRect,
} from "../lib/backend";
import { notifyDocumentChanged } from "../lib/documentEvents";

/** Matches the backend's minimum free text box, in display points. */
const MIN_SIZE = 12;
/** Pointer travel below this is a click, not a transform. */
const MOVE_SLOP = 0.5;
const DEFAULT_COLOR = "#111827";
const DEFAULT_FONT_SIZE = 12;
const DEFAULT_OPACITY = 100;
const FONT_SIZE_MIN = 6;
const FONT_SIZE_MAX = 72;

const KIND_LABELS: Record<FreeTextKind, string> = {
  freetext: "自由文本",
  callout: "标注框",
};

const ALIGN_LABELS: Record<FreeTextAlign, string> = {
  left: "左对齐",
  center: "居中",
  right: "右对齐",
};

type Point = { x: number; y: number };
type Notice = { kind: "info" | "error"; text: string };

type Interaction =
  | {
      kind: "create";
      freeTextKind: FreeTextKind;
      anchor: Point;
      current: Point;
    }
  | {
      kind: "move";
      annotation: AnnotationView;
      start: Point;
      current: Point;
    }
  | { kind: "resize"; annotation: AnnotationView; current: Point };

export interface FreeTextLayerProps {
  documentId: string;
  pageIndex: number;
  /** Display-space page size, matching the page render coordinate system. */
  pageWidth: number;
  pageHeight: number;
  /** Permission Restrictions deny every editing capability. */
  readOnly?: boolean;
  /** The PDF security handler allows adding and modifying annotations. */
  canAnnotate?: boolean;
  annotations?: AnnotationView[];
  selectedAnnotation?: AnnotationView | null;
  onSelectAnnotation?: (annotation: AnnotationView | null) => void;
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(Math.max(value, minimum), maximum);
}

function dragRect(
  anchor: Point,
  current: Point,
  pageWidth: number,
  pageHeight: number,
): TextMarkupRect {
  const left = clamp(Math.min(anchor.x, current.x), 0, pageWidth);
  const right = clamp(Math.max(anchor.x, current.x), 0, pageWidth);
  const top = clamp(Math.min(anchor.y, current.y), 0, pageHeight);
  const bottom = clamp(Math.max(anchor.y, current.y), 0, pageHeight);
  return { x: left, y: top, width: right - left, height: bottom - top };
}

function moveRect(
  rect: TextMarkupRect,
  dx: number,
  dy: number,
  pageWidth: number,
  pageHeight: number,
): TextMarkupRect {
  return {
    ...rect,
    x: clamp(rect.x + dx, 0, Math.max(0, pageWidth - rect.width)),
    y: clamp(rect.y + dy, 0, Math.max(0, pageHeight - rect.height)),
  };
}

function resizeRect(
  rect: TextMarkupRect,
  point: Point,
  pageWidth: number,
  pageHeight: number,
): TextMarkupRect {
  return {
    ...rect,
    width: clamp(point.x - rect.x, MIN_SIZE, Math.max(MIN_SIZE, pageWidth - rect.x)),
    height: clamp(
      point.y - rect.y,
      MIN_SIZE,
      Math.max(MIN_SIZE, pageHeight - rect.y),
    ),
  };
}

function rectsEqual(left: TextMarkupRect, right: TextMarkupRect): boolean {
  return (
    Math.abs(left.x - right.x) < MOVE_SLOP &&
    Math.abs(left.y - right.y) < MOVE_SLOP &&
    Math.abs(left.width - right.width) < MOVE_SLOP &&
    Math.abs(left.height - right.height) < MOVE_SLOP
  );
}

function percent(value: number, total: number): string {
  return `${total > 0 ? (value / total) * 100 : 0}%`;
}

function rectStyle(
  rect: TextMarkupRect,
  pageWidth: number,
  pageHeight: number,
): CSSProperties {
  return {
    left: percent(rect.x, pageWidth),
    top: percent(rect.y, pageHeight),
    width: percent(rect.width, pageWidth),
    height: percent(rect.height, pageHeight),
  };
}

/**
 * Canvas overlay for native free text and callout annotations.
 *
 * Drag with the "自由文本" or "标注框" tool to place a box, then type in the
 * inline editor. Double-click (or the toolbar's edit button) reopens the
 * editor; dragging the box moves it and its corner handle resizes it. Every
 * completed gesture and text edit is one unified-history action, and the
 * overlay never renders text itself: the saved `/AP` is already in the page
 * bitmap, so the overlay only adds hit areas and chrome.
 */
export function FreeTextLayer({
  documentId,
  pageIndex,
  pageWidth,
  pageHeight,
  readOnly = false,
  canAnnotate = false,
  annotations = [],
  selectedAnnotation = null,
  onSelectAnnotation,
}: FreeTextLayerProps) {
  const [tool, setTool] = useState<FreeTextKind | null>(null);
  const [color, setColor] = useState(DEFAULT_COLOR);
  const [opacity, setOpacity] = useState(DEFAULT_OPACITY);
  const [fontSize, setFontSize] = useState(DEFAULT_FONT_SIZE);
  const [align, setAlign] = useState<FreeTextAlign>("left");
  const [interaction, setInteraction] = useState<Interaction | null>(null);
  const [editingName, setEditingName] = useState<string | null>(null);
  const [textDraft, setTextDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const layerRef = useRef<HTMLDivElement>(null);

  const canEdit = canAnnotate && !readOnly;

  const pageAnnotations = useMemo(
    () =>
      annotations.filter(
        (annotation) =>
          annotation.pageIndex === pageIndex && annotation.freeText !== null,
      ),
    [annotations, pageIndex],
  );

  const selected =
    selectedAnnotation?.pageIndex === pageIndex &&
    selectedAnnotation.freeText !== null
      ? selectedAnnotation
      : null;
  const editing = editingName
    ? (pageAnnotations.find((annotation) => annotation.name === editingName) ??
      null)
    : null;

  useEffect(() => {
    if (!canEdit) {
      setTool(null);
      setInteraction(null);
      setEditingName(null);
    }
  }, [canEdit]);

  useEffect(() => {
    setTool(null);
    setInteraction(null);
    setEditingName(null);
    setNotice(null);
  }, [documentId, pageIndex]);

  // Selecting another annotation closes the editor instead of letting it
  // silently commit onto the wrong box.
  useEffect(() => {
    if (
      editingName !== null &&
      selectedAnnotation !== null &&
      selectedAnnotation.name !== editingName
    ) {
      setEditingName(null);
    }
  }, [editingName, selectedAnnotation]);

  useEffect(() => {
    if (!tool && !interaction && editingName === null) {
      return;
    }
    const cancel = (event: KeyboardEvent) => {
      if (event.key !== "Escape") {
        return;
      }
      event.preventDefault();
      setInteraction(null);
      setTool(null);
      setEditingName(null);
    };
    window.addEventListener("keydown", cancel);
    return () => window.removeEventListener("keydown", cancel);
  }, [tool, interaction, editingName]);

  const pointFromEvent = (event: {
    clientX: number;
    clientY: number;
  }): Point => {
    const bounds = layerRef.current?.getBoundingClientRect();
    const left = bounds?.left ?? 0;
    const top = bounds?.top ?? 0;
    const width = bounds && bounds.width > 0 ? bounds.width : pageWidth;
    const height = bounds && bounds.height > 0 ? bounds.height : pageHeight;
    return {
      x: ((event.clientX - left) / width) * pageWidth,
      y: ((event.clientY - top) / height) * pageHeight,
    };
  };

  function capturePointer(event: ReactPointerEvent<Element>) {
    if (typeof event.currentTarget.setPointerCapture === "function") {
      try {
        event.currentTarget.setPointerCapture(event.pointerId);
      } catch {
        // Pointer capture is unavailable in older webviews; bubbling events
        // inside the overlay are still enough for a normal drag.
      }
    }
  }

  function startCreate(event: ReactPointerEvent<HTMLDivElement>) {
    if (!canEdit || busy || tool === null || event.button !== 0) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    capturePointer(event);
    const point = pointFromEvent(event);
    setNotice(null);
    setInteraction({
      kind: "create",
      freeTextKind: tool,
      anchor: point,
      current: point,
    });
  }

  function startMove(
    event: ReactPointerEvent<HTMLDivElement>,
    annotation: AnnotationView,
  ) {
    event.stopPropagation();
    onSelectAnnotation?.(annotation);
    if (!canEdit || busy || tool || event.button !== 0) {
      return;
    }
    event.preventDefault();
    capturePointer(event);
    const point = pointFromEvent(event);
    setNotice(null);
    setInteraction({
      kind: "move",
      annotation,
      start: point,
      current: point,
    });
  }

  function startResize(
    event: ReactPointerEvent<HTMLButtonElement>,
    annotation: AnnotationView,
  ) {
    if (!canEdit || busy || tool) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    capturePointer(event);
    setInteraction({
      kind: "resize",
      annotation,
      current: pointFromEvent(event),
    });
  }

  function moveInteraction(event: ReactPointerEvent<Element>) {
    if (!interaction) {
      return;
    }
    setInteraction({ ...interaction, current: pointFromEvent(event) });
  }

  function previewRect(interaction: Interaction): TextMarkupRect | null {
    if (interaction.kind === "create") {
      return dragRect(interaction.anchor, interaction.current, pageWidth, pageHeight);
    }
    const rect = interaction.annotation.rects[0];
    if (!rect) {
      return null;
    }
    if (interaction.kind === "move") {
      return moveRect(
        rect,
        interaction.current.x - interaction.start.x,
        interaction.current.y - interaction.start.y,
        pageWidth,
        pageHeight,
      );
    }
    return resizeRect(rect, interaction.current, pageWidth, pageHeight);
  }

  async function createFromInteraction(
    interaction: Extract<Interaction, { kind: "create" }>,
  ) {
    const rect = dragRect(
      interaction.anchor,
      interaction.current,
      pageWidth,
      pageHeight,
    );
    setInteraction(null);
    if (rect.width < MIN_SIZE || rect.height < MIN_SIZE) {
      setNotice({
        kind: "info",
        text: `文本框至少需要 ${MIN_SIZE} × ${MIN_SIZE} pt`,
      });
      return;
    }
    setBusy(true);
    setNotice(null);
    try {
      const result = await createFreeTextAnnotation(documentId, {
        pageIndex,
        kind: interaction.freeTextKind,
        rect,
        color,
        opacity,
        text: "",
        fontSize,
        align,
      });
      notifyDocumentChanged({ documentId, pageIndex, source: "annotation" });
      onSelectAnnotation?.(result.annotation);
      setTool(null);
      if (result.annotation) {
        setEditingName(result.annotation.name);
        setTextDraft("");
      }
    } catch (error) {
      setNotice({ kind: "error", text: toCommandError(error).message });
    } finally {
      setBusy(false);
    }
  }

  async function commitTransform(
    annotation: AnnotationView,
    rect: TextMarkupRect,
  ) {
    setInteraction(null);
    const current = annotation.rects[0];
    if (!current || rectsEqual(current, rect)) {
      return;
    }
    setBusy(true);
    setNotice(null);
    try {
      const result = await transformFreeTextAnnotation(
        documentId,
        annotation.name,
        rect,
      );
      notifyDocumentChanged({ documentId, pageIndex, source: "annotation" });
      onSelectAnnotation?.(result.annotation);
    } catch (error) {
      setNotice({ kind: "error", text: toCommandError(error).message });
    } finally {
      setBusy(false);
    }
  }

  function endInteraction() {
    if (!interaction) {
      return;
    }
    if (interaction.kind === "create") {
      void createFromInteraction(interaction);
      return;
    }
    const rect = previewRect(interaction);
    if (rect) {
      void commitTransform(interaction.annotation, rect);
    } else {
      setInteraction(null);
    }
  }

  function beginEdit(annotation: AnnotationView) {
    if (!canEdit || busy) {
      return;
    }
    onSelectAnnotation?.(annotation);
    setTextDraft(annotation.text ?? "");
    setEditingName(annotation.name);
    setNotice(null);
  }

  async function commitEdit() {
    if (!editing || busy) {
      return;
    }
    const original = editing.text ?? "";
    if (textDraft === original) {
      setEditingName(null);
      return;
    }
    const view = editing.freeText;
    setBusy(true);
    setNotice(null);
    try {
      const result = await updateFreeText(documentId, {
        name: editing.name,
        text: textDraft,
        fontSize: view?.fontSize ?? DEFAULT_FONT_SIZE,
        align: view?.align ?? "left",
        color: editing.color,
        opacity: editing.opacity,
      });
      notifyDocumentChanged({ documentId, pageIndex, source: "annotation" });
      onSelectAnnotation?.(result.annotation);
      setEditingName(null);
    } catch (error) {
      setNotice({ kind: "error", text: toCommandError(error).message });
    } finally {
      setBusy(false);
    }
  }

  if (!canEdit) {
    return null;
  }

  const selectedRect = selected?.rects[0] ?? null;
  const preview =
    interaction && interaction.kind !== "create" && interaction.annotation.name
      ? previewRect(interaction)
      : null;
  const createPreview =
    interaction?.kind === "create" ? previewRect(interaction) : null;
  const selectedAnnotationRect =
    selected && preview && interaction?.kind !== "create"
      ? preview
      : selectedRect;

  return (
    <div ref={layerRef} className="free-text-layer" aria-label="自由文本层">
      {pageAnnotations.map((annotation) => {
        const rect = annotation.rects[0];
        if (!rect) {
          return null;
        }
        const isSelected = selected?.name === annotation.name;
        return (
          <div
            key={annotation.name}
            className={`free-text-hit${isSelected ? " free-text-hit--selected" : ""}`}
            data-testid={`free-text-hit-${annotation.name}`}
            aria-label={`${KIND_LABELS[annotation.freeText?.kind ?? "freetext"]}，${annotation.text?.trim() || "空白"}`}
            style={rectStyle(rect, pageWidth, pageHeight)}
            onPointerDown={(event) => startMove(event, annotation)}
            onPointerMove={moveInteraction}
            onPointerUp={endInteraction}
            onPointerCancel={endInteraction}
            onDoubleClick={(event) => {
              event.stopPropagation();
              beginEdit(annotation);
            }}
          />
        );
      })}

      {createPreview && (
        <div
          className="free-text-create-preview"
          data-testid="free-text-create-preview"
          aria-hidden="true"
          style={rectStyle(createPreview, pageWidth, pageHeight)}
        />
      )}

      {selectedAnnotationRect && preview && (
        <div
          className="free-text-transform-preview"
          data-testid="free-text-transform-preview"
          aria-hidden="true"
          style={rectStyle(selectedAnnotationRect, pageWidth, pageHeight)}
        />
      )}

      {selectedAnnotationRect && !tool && (
        <button
          type="button"
          className="free-text-resize-handle"
          aria-label="调整文本框大小"
          data-testid="free-text-resize-handle"
          style={{
            left: percent(
              selectedAnnotationRect.x + selectedAnnotationRect.width,
              pageWidth,
            ),
            top: percent(
              selectedAnnotationRect.y + selectedAnnotationRect.height,
              pageHeight,
            ),
          }}
          onPointerDown={(event) => {
            if (selected) {
              startResize(event, selected);
            }
          }}
          onPointerMove={moveInteraction}
          onPointerUp={endInteraction}
          onPointerCancel={endInteraction}
        />
      )}

      {tool && (
        <div
          className="free-text-draw-surface"
          data-testid="free-text-draw-surface"
          onPointerDown={startCreate}
          onPointerMove={moveInteraction}
          onPointerUp={endInteraction}
          onPointerCancel={endInteraction}
          onContextMenu={(event) => event.preventDefault()}
        />
      )}

      {editing && editing.rects[0] && (
        <div
          className="free-text-editor"
          style={rectStyle(editing.rects[0], pageWidth, pageHeight)}
          onPointerDown={(event) => event.stopPropagation()}
        >
          <textarea
            aria-label="自由文本编辑器"
            value={textDraft}
            maxLength={5000}
            autoFocus
            disabled={busy}
            onChange={(event) => setTextDraft(event.target.value)}
            onKeyDown={(event) => {
              event.stopPropagation();
              if (event.key === "Escape") {
                event.preventDefault();
                setEditingName(null);
              } else if (
                event.key === "Enter" &&
                (event.metaKey || event.ctrlKey)
              ) {
                event.preventDefault();
                void commitEdit();
              }
            }}
          />
          <div className="free-text-editor-actions">
            <button
              type="button"
              disabled={busy}
              onClick={() => void commitEdit()}
            >
              保存文本
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => setEditingName(null)}
            >
              取消
            </button>
          </div>
        </div>
      )}

      <div
        className="free-text-toolbar"
        role="toolbar"
        aria-label="自由文本工具"
        onPointerDown={(event) => event.stopPropagation()}
      >
        <span className="free-text-toolbar-title">文本</span>
        {(Object.keys(KIND_LABELS) as FreeTextKind[]).map((value) => (
          <button
            key={value}
            type="button"
            aria-pressed={tool === value}
            disabled={busy}
            onClick={() => {
              setNotice(null);
              setInteraction(null);
              setEditingName(null);
              setTool((current) => (current === value ? null : value));
            }}
          >
            {KIND_LABELS[value]}
          </button>
        ))}
        <button
          type="button"
          aria-label="编辑文本"
          disabled={selected === null || busy}
          onClick={() => {
            if (selected) {
              beginEdit(selected);
            }
          }}
        >
          编辑文本
        </button>
        <label className="free-text-toolbar-control">
          字号
          <input
            type="number"
            aria-label="文本字号"
            min={FONT_SIZE_MIN}
            max={FONT_SIZE_MAX}
            step={1}
            value={fontSize}
            disabled={busy}
            onChange={(event) => {
              const next = Number.parseFloat(event.target.value);
              if (Number.isFinite(next)) {
                setFontSize(clamp(next, FONT_SIZE_MIN, FONT_SIZE_MAX));
              }
            }}
          />
        </label>
        <input
          type="color"
          aria-label="文本颜色"
          value={color}
          disabled={busy}
          onChange={(event) => setColor(event.target.value)}
        />
        <select
          aria-label="文本对齐"
          value={align}
          disabled={busy}
          onChange={(event) => setAlign(event.target.value as FreeTextAlign)}
        >
          {(Object.entries(ALIGN_LABELS) as [FreeTextAlign, string][]).map(
            ([value, label]) => (
              <option key={value} value={value}>
                {label}
              </option>
            ),
          )}
        </select>
        <label className="free-text-toolbar-control">
          透明度
          <input
            type="range"
            aria-label="文本透明度"
            min={0}
            max={100}
            step={1}
            value={opacity}
            disabled={busy}
            onChange={(event) =>
              setOpacity(Number.parseInt(event.target.value, 10))
            }
          />
          <span>{opacity}%</span>
        </label>
        {notice && (
          <span
            className={`text-edit-notice text-edit-notice--${notice.kind}`}
            role={notice.kind === "error" ? "alert" : "status"}
          >
            {notice.text}
          </span>
        )}
      </div>
    </div>
  );
}
