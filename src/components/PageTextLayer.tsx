import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import {
  applyTextBoxSelectionStyle,
  applyTextBoxStyle,
  applyTextEdit,
  createInkAnnotation,
  createLink,
  createNoteAnnotation,
  createTextBox,
  createTextMarkupAnnotation,
  deleteTextBox,
  editHistory,
  extractTextLayer,
  redoTextEdit,
  previewTextBoxTransform,
  textBoxFonts,
  textBoxes,
  textEditTarget,
  toCommandError,
  transformNoteAnnotation,
  transformTextBox,
  undoTextEdit,
  type AnnotationView,
  type CommandError,
  type EditHistoryView,
  type FontUsageView,
  type InkPoint,
  type LinkTarget,
  type TextBoxFontView,
  type TextBoxStyleView,
  type TextBoxTargetView,
  type TextLayerView,
  type TextMarkupKind,
  type TextMarkupRect,
  type TextRangeView,
  type TextBoxTransformPreview,
  type TextBoxMovePreview,
} from "../lib/backend";
import {
  notifyDocumentChanged,
  onDocumentChanged,
} from "../lib/documentEvents";
import { useTextMovePreview } from "../lib/useTextMovePreview";
import { useTextEditorFont } from "../lib/useTextEditorFont";
import { linkErrorText } from "../lib/linkTarget";
import {
  dragRect,
  estimatedContentHeight,
  fitBoxToPage,
  moveBox,
  resizeBox,
  resizeBoxFromDrag,
  contentSelectionFrame,
  RESIZE_HANDLES,
  type ResizeHandle,
  rotationFromPointer,
  type BoxRect,
  type Point as BoxPoint,
} from "../lib/textBoxGeometry";
import { TextBoxStylePanel } from "./TextBoxStylePanel";
import { LinkTargetDialog } from "./LinkTargetDialog";
import {
  registerPendingEdit,
  notifyPendingEditChanged,
} from "../lib/pendingEdits";
import { TextStylePlaceholder } from "./TextStylePlaceholder";
import { ToolShelf, useEditorTools, type ToolShelfTool } from "./ToolShelf";
import { WorkspaceIcon } from "./WorkspaceIcon";
import {
  buildTextLayer,
  editableTextBlocks,
  findCharAt,
  selectionForChar,
  selectionForDrag,
  selectionForLine,
  selectionForWord,
  selectionRects,
  selectionText,
  type TextLayerStructure,
  type TextPoint,
  type TextRange,
} from "../lib/textLayer";

type LayerState =
  | { status: "loading" }
  | { status: "ready"; layer: TextLayerView }
  | { status: "error"; error: CommandError };

type EditorState =
  | { status: "loading" }
  | {
      status: "editing";
      target: TextBoxTargetView;
      draft: string;
      busy: boolean;
      error: string | null;
    };

type Notice = { kind: "info" | "error"; text: string };

type BoxInteraction =
  | {
      kind: "create";
      anchor: BoxPoint;
      current: BoxPoint;
    }
  | {
      kind: "move";
      box: TextBoxTargetView;
      start: BoxPoint;
      current: BoxPoint;
    }
  | {
      kind: "resize";
      handle?: ResizeHandle;
      box: TextBoxTargetView;
      start: BoxPoint;
      current: BoxPoint;
    }
  | {
      kind: "rotate";
      box: TextBoxTargetView;
      start: BoxPoint;
      current: BoxPoint;
    };

type NoteInteraction = {
  kind: "move" | "resize";
  annotation: AnnotationView;
  start: InkPoint;
  current: InkPoint;
};

const READ_ONLY_REASON = "此文档不允许编辑内容";
const SCANNED_REASON = "此页没有可编辑文字";
const ANNOTATION_PERMISSION_REASON = "此文档不允许添加注释";
const DEFAULT_MARKUP_COLOR = "#ffd400";
const DEFAULT_MARKUP_OPACITY = 50;
const DEFAULT_NOTE_COLOR = "#f9a825";
const DEFAULT_NOTE_SIZE = 28;
const MIN_NOTE_SIZE = 12;
const DEFAULT_INK_COLOR = "#c62828";
const DEFAULT_INK_OPACITY = 80;
const DEFAULT_INK_LINE_WIDTH = 3;

/** The one Chinese hint the canvas shows when a font falls back. */
function fontFallbackNotice(font: FontUsageView | null): Notice | null {
  if (!font?.fallback) {
    return null;
  }
  return {
    kind: "info",
    text: `${font.originalFamily} 缺少部分新文字的字形，这些字已改用 ${font.family}`,
  };
}

/**
 * Converts a textarea selection to the code-point range runs use.
 *
 * The DOM counts UTF-16 code units while the backend counts Unicode scalar
 * values, so a run after an emoji would be off by one without this conversion.
 * A collapsed selection returns `null`, which means "style the whole box".
 */
function codePointRange(
  text: string,
  start: number,
  end: number,
): TextRangeView | null {
  const unitStart = Math.max(0, Math.min(start, end, text.length));
  const unitEnd = Math.min(text.length, Math.max(start, end));
  if (unitStart === unitEnd) {
    return null;
  }
  return {
    start: [...text.slice(0, unitStart)].length,
    end: [...text.slice(0, unitEnd)].length,
  };
}

export interface PageTextLayerProps {
  documentId: string;
  pageIndex: number;
  /** Permission Restrictions forbid changing document content. */
  readOnly?: boolean;
  /** The PDF security handler allows adding and modifying annotations. */
  canAnnotate?: boolean;
  /** Annotations on this page, used for note overlays and the selection outline. */
  annotations?: AnnotationView[];
  /** Annotation selected in the document-wide list, outlined on its page. */
  selectedAnnotation?: AnnotationView | null;
  /** Selects an annotation in the document-wide list's model. */
  onSelectAnnotation?: (annotation: AnnotationView | null) => void;
  /** Page render scale, used to size the inline editor text. */
  scale?: number;
  /** Number of pages an internal link may address. */
  pageCount?: number;
  /** Advances when the committed page bitmap has actually loaded. */
  paintVersion?: number;
}

function TextEditToolbar({
  canEdit,
  canAnnotate,
  editMode,
  boxMode,
  noteMode,
  inkMode,
  inkColor,
  inkOpacity,
  inkLineWidth,
  readOnly,
  busy,
  notice,
  onToggleEdit,
  onToggleBoxMode,
  onToggleNoteMode,
  onToggleInkMode,
  onInkColor,
  onInkOpacity,
  onInkLineWidth,
  stylePanel,
  history,
  onUndo,
  onRedo,
}: {
  canEdit: boolean;
  canAnnotate: boolean;
  editMode: boolean;
  boxMode: boolean;
  noteMode: boolean;
  inkMode: boolean;
  inkColor: string;
  inkOpacity: number;
  inkLineWidth: number;
  readOnly: boolean;
  busy: boolean;
  history: EditHistoryView | null;
  notice: Notice | null;
  onToggleEdit: () => void;
  onToggleBoxMode: () => void;
  onToggleNoteMode: () => void;
  onToggleInkMode: () => void;
  onInkColor: (color: string) => void;
  onInkOpacity: (opacity: number) => void;
  onInkLineWidth: (width: number) => void;
  onUndo: () => void;
  onRedo: () => void;
  stylePanel: ReactNode;
}) {
  const hosted = useEditorTools();
  const annotationDisabled = !canAnnotate || readOnly || busy;
  const editDisabled = !canEdit || readOnly || busy;
  const editTitle = readOnly
    ? READ_ONLY_REASON
    : !canEdit
      ? SCANNED_REASON
      : editMode
        ? "退出文字编辑"
        : "点击页面上的文字开始编辑";
  const boxTitle = readOnly
    ? READ_ONLY_REASON
    : !canEdit
      ? SCANNED_REASON
      : boxMode
        ? "退出文本框编辑"
        : "点击原有文字以选中，或在空白处拖动以添加文本框";
  const annotationTitle = (armed: boolean, idle: string) =>
    readOnly
      ? READ_ONLY_REASON
      : !canAnnotate
        ? ANNOTATION_PERMISSION_REASON
        : armed
          ? "退出该注释工具"
          : idle;
  const tools: ToolShelfTool[] = [
    {
      id: "edit-text",
      label: hosted ? "编辑内容" : "编辑文字",
      icon: "textEdit",
      groups: ["text"],
      active: editMode,
      disabled: editDisabled,
      title: editTitle,
      order: 3,
      onActivate: onToggleEdit,
    },
    {
      id: "text-box",
      label: hosted ? "添加段落" : "添加文本框",
      icon: "textBox",
      groups: ["text"],
      active: boxMode,
      disabled: readOnly || busy,
      title: boxTitle,
      order: 1,
      onActivate: onToggleBoxMode,
    },
    {
      id: "comment-text",
      label: "添加文本",
      icon: "textBox",
      groups: ["comment"],
      active: boxMode,
      disabled: readOnly || busy,
      title: boxTitle,
      order: 1,
      onActivate: onToggleBoxMode,
    },
    {
      id: "note",
      label: "便签",
      icon: "note",
      groups: ["annotate"],
      active: noteMode,
      disabled: annotationDisabled,
      title: annotationTitle(noteMode, "在页面上点击以放置便签"),
      order: 1,
      onActivate: onToggleNoteMode,
    },
    {
      id: "ink",
      label: "手绘",
      icon: "pen",
      groups: ["annotate"],
      active: inkMode,
      disabled: annotationDisabled,
      title: annotationTitle(inkMode, "在页面上按住并拖动以手绘"),
      order: 2,
      onActivate: onToggleInkMode,
    },
    {
      id: "comment-ink",
      label: "书写",
      icon: "pen",
      groups: ["comment"],
      active: inkMode,
      disabled: annotationDisabled,
      title: annotationTitle(inkMode, "在页面上按住并拖动以手绘"),
      order: 3,
      onActivate: onToggleInkMode,
    },
  ];
  return (
    <ToolShelf tools={tools} panelTitle="文本样式" defaultToolId="edit-text">
      <div
        className="text-edit-toolbar"
        onPointerDown={(event) => event.stopPropagation()}
      >
        {inkMode && (
          <span className="annotation-ink-controls">
            <label className="text-markup-control">
              颜色
              <input
                type="color"
                aria-label="画笔颜色"
                value={inkColor}
                disabled={busy}
                onChange={(event) => onInkColor(event.target.value)}
              />
            </label>
            <label className="text-markup-control">
              透明度
              <input
                type="range"
                aria-label="画笔透明度"
                min={0}
                max={100}
                step={1}
                value={inkOpacity}
                disabled={busy}
                onChange={(event) =>
                  onInkOpacity(Number.parseInt(event.target.value, 10))
                }
              />
              <span>{inkOpacity}%</span>
            </label>
            <label className="text-markup-control">
              线宽
              <input
                type="range"
                aria-label="画笔线宽"
                min={0.5}
                max={32}
                step={0.5}
                value={inkLineWidth}
                disabled={busy}
                onChange={(event) =>
                  onInkLineWidth(Number.parseFloat(event.target.value))
                }
              />
              <span>{inkLineWidth} pt</span>
            </label>
          </span>
        )}
        {!hosted && (
          <>
            <button
              type="button"
              disabled={!history?.canUndo || busy}
              onClick={onUndo}
            >
              撤销
            </button>
            <button
              type="button"
              disabled={!history?.canRedo || busy}
              onClick={onRedo}
            >
              重做
            </button>
          </>
        )}
        {readOnly && (
          <span className="text-edit-reason">{READ_ONLY_REASON}</span>
        )}
        {notice && (
          <span
            className={`text-edit-notice text-edit-notice--${notice.kind}`}
            role={notice.kind === "error" ? "alert" : "status"}
          >
            {notice.text}
          </span>
        )}
        {hosted && !stylePanel && (editMode || boxMode) && (
          <TextStylePlaceholder />
        )}
        {stylePanel}
      </div>
    </ToolShelf>
  );
}

/**
 * Transparent interaction layer over the rendered page image.
 *
 * Geometry from the backend is in display-space page points, so highlight
 * rectangles and the Text Box editor are positioned as percentages of the
 * page box. That keeps the layer aligned at every zoom level without
 * re-fetching text when zoom changes, and the page rotation/CropBox are
 * already applied by the backend.
 *
 * In selection mode clicks select and copy text. In edit mode a click resolves
 * the original Text Object through `text_edit_target` and opens an inline Text
 * Box editor prefilled with the original content, font, size and colour.
 * Text Box mode adds its own overlay: dragging empty page space creates a box,
 * and selecting a box offers move, resize, rotate and whole-box style controls.
 */
export function PageTextLayer({
  documentId,
  pageIndex,
  readOnly = false,
  canAnnotate = false,
  annotations = [],
  selectedAnnotation = null,
  onSelectAnnotation,
  scale = 1,
  pageCount = 0,
  paintVersion,
}: PageTextLayerProps) {
  const workspaceTools = useEditorTools();
  const [state, setState] = useState<LayerState>({ status: "loading" });
  const [selection, setSelection] = useState<TextRange | null>(null);
  const [copyStatus, setCopyStatus] = useState<"idle" | "copied" | "failed">(
    "idle",
  );
  const [markupColor, setMarkupColor] = useState(DEFAULT_MARKUP_COLOR);
  const [markupOpacity, setMarkupOpacity] = useState(DEFAULT_MARKUP_OPACITY);
  const [markupBusy, setMarkupBusy] = useState(false);
  const [linkDraft, setLinkDraft] = useState<{
    rects: TextMarkupRect[];
  } | null>(null);
  const [linkBusy, setLinkBusy] = useState(false);
  const [linkError, setLinkError] = useState<string | null>(null);
  const [noteMode, setNoteMode] = useState(false);
  const [inkMode, setInkMode] = useState(false);
  const [inkColor, setInkColor] = useState(DEFAULT_INK_COLOR);
  const [inkOpacity, setInkOpacity] = useState(DEFAULT_INK_OPACITY);
  const [inkLineWidth, setInkLineWidth] = useState(DEFAULT_INK_LINE_WIDTH);
  const [inkDraft, setInkDraft] = useState<InkPoint[] | null>(null);
  const [inkBusy, setInkBusy] = useState(false);
  const [noteInteraction, setNoteInteraction] =
    useState<NoteInteraction | null>(null);
  const [noteBusy, setNoteBusy] = useState(false);
  const [editMode, setEditMode] = useState(false);
  const [boxMode, setBoxMode] = useState(false);
  const [boxes, setBoxes] = useState<TextBoxTargetView[]>([]);
  const [fonts, setFonts] = useState<TextBoxFontView[]>([]);
  const [selectedBoxId, setSelectedBoxId] = useState<number | null>(null);
  const [boxInteraction, setBoxInteractionState] = useState<BoxInteraction | null>(null);
  const interactionRef = useRef<BoxInteraction | null>(null);
  const motionFrame = useRef<number | null>(null);
  function setBoxInteraction(next: BoxInteraction | null) {
    if (motionFrame.current !== null) cancelAnimationFrame(motionFrame.current);
    motionFrame.current = null;
    interactionRef.current = next;
    setBoxInteractionState(next);
  }
  function queueBoxInteraction(next: BoxInteraction) {
    interactionRef.current = next;
    if (motionFrame.current !== null) return;
    motionFrame.current = requestAnimationFrame(() => {
      motionFrame.current = null;
      setBoxInteractionState(interactionRef.current);
    });
  }
  useEffect(() => () => {
    if (motionFrame.current !== null) cancelAnimationFrame(motionFrame.current);
  }, []);
  const [boxBusy, setBoxBusy] = useState(false);
  const [dragPreview, setDragPreview] =
    useState<TextBoxTransformPreview | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const paintVersionRef = useRef(paintVersion);
  paintVersionRef.current = paintVersion;
  const [pendingMove, setPendingMove] = useState<{
    boxId: number; data: TextBoxMovePreview; rect: BoxRect; origin: BoxRect; afterPaint: number | null;
  } | null>(null);
  useEffect(() => {
    if (pendingMove?.afterPaint != null && paintVersion !== undefined && paintVersion > pendingMove.afterPaint) setPendingMove(null);
  }, [paintVersion, pendingMove]);
  const previewScale = Math.min(2, Math.max(0.1, scale * (window.devicePixelRatio || 1)));
  const cacheBox = (editMode || Boolean(workspaceTools && boxMode)) && !editor && !boxBusy
    ? boxes.find(box => box.boxId === selectedBoxId) ?? null : null;
  const moveCache = useTextMovePreview(documentId, pageIndex, cacheBox, reloadKey, previewScale);
  const previewing =
    (editMode || Boolean(workspaceTools)) &&
    (boxInteraction?.kind === "resize" || (boxInteraction?.kind === "move" && moveCache.status === "fallback"));
  useEffect(() => {
    if (!previewing) {
      setDragPreview(null);
      return;
    }
    let alive = true;
    let timer: ReturnType<typeof setTimeout>;
    let previous = "";
    async function draw() {
      const gesture = interactionRef.current;
      if (
        !alive ||
        !gesture ||
        (gesture.kind !== "move" && gesture.kind !== "resize")
      )
        return;
      const box = gesture.box;
      if (
        Math.hypot(
          gesture.current.x - gesture.start.x,
          gesture.current.y - gesture.start.y,
        ) <
        1 / scale
      ) {
        if (alive) {
          setDragPreview(null);
          timer = setTimeout(() => void draw(), 16);
        }
        return;
      }
      const rect =
        gesture.kind === "move"
          ? moveBox(
              box,
              {
                x: gesture.current.x - gesture.start.x,
                y: gesture.current.y - gesture.start.y,
              },
              state.status === "ready" ? state.layer.pageWidth : 10000,
              state.status === "ready" ? state.layer.pageHeight : 10000,
            )
          : resizeBoxFromDrag(
              box,
              box.rotation,
              gesture.start,
              gesture.current,
              gesture.handle ?? "se",
            );
      if (
        gesture.kind === "resize" &&
        (gesture.handle === "e" || gesture.handle === "w") &&
        Math.abs(rect.width - box.width) > 0.01
      )
        rect.height = box.fontSize * 1.2;
      const key = `${rect.x}:${rect.y}:${rect.width}:${rect.height}`;
      if (key !== previous) {
        previous = key;
        try {
          const next = await previewTextBoxTransform(
            documentId,
            pageIndex,
            box.boxId,
            {
              ...rect,
              rotation: box.rotation,
              autoHeight: gesture.kind === "resize" || box.autoHeight,
            },
            Math.min(2, Math.max(0.1, scale * (window.devicePixelRatio || 1))),
          );
          if (alive) setDragPreview(next);
        } catch {
          /* The final transform reports any persistent engine error. */
        }
      }
      if (alive) timer = setTimeout(() => void draw(), 16);
    }
    void draw();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [previewing, documentId, pageIndex, scale]);
  // Textarea selection offsets in UTF-16 units, or null when the caret is
  // collapsed. They are converted to code points at the command boundary.
  const [editorSelection, setEditorSelection] = useState<{
    start: number;
    end: number;
  } | null>(null);
  const [history, setHistory] = useState<EditHistoryView | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  // External overlays (shapes, the annotation list) change the same document
  // and share this undo stack, so their events re-fetch the history. This
  // layer's own commands already return the new history, and re-fetching it
  // here could overwrite that with a slower, stale response.
  const [historyRefreshKey, setHistoryRefreshKey] = useState(0);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const editorFont = useTextEditorFont(documentId, pageIndex,
    editor?.status === "editing" ? editor.target : null,
    editor?.status === "editing" ? editor.draft : "");
  const selectNewParagraph = useRef(false);
  const dragAnchor = useRef<number | null>(null);
  const targetRequest = useRef(0);
  const enterEditingAfterResolve = useRef(false);
  const loadedPageKey = useRef<string | null>(null);
  const draftDirty = useRef(false);
  draftDirty.current =
    editor?.status === "editing" && editor.draft !== editor.target.text;
  useEffect(() => notifyPendingEditChanged(), [editor]);
  const pendingCommit = useRef<() => Promise<boolean>>(async () => true);
  useEffect(() => {
    if (editor?.status !== "editing") return;
    return registerPendingEdit(
      documentId,
      () => pendingCommit.current(),
      () => draftDirty.current,
    );
  }, [documentId, editor?.status]);

  // Re-fetch the layer when this page's content changes (edit, delete, undo,
  // redo). The existing layer stays visible during the refresh so the overlay
  // does not blink; only a document/page change shows the loading state.
  useEffect(() => {
    let cancelled = false;
    const pageKey = `${documentId}:${pageIndex}`;
    const samePage = loadedPageKey.current === pageKey;
    loadedPageKey.current = pageKey;

    if (!samePage) {
      setState({ status: "loading" });
      setSelection(null);
      setCopyStatus("idle");
      setEditor(null);
      setEditMode(false);
      setBoxMode(false);
      setNoteMode(false);
      setInkMode(false);
      setInkDraft(null);
      setNoteInteraction(null);
      setBoxes([]);
      setSelectedBoxId(null);
      setBoxInteraction(null);
      setNotice(null);
    }

    extractTextLayer(documentId, pageIndex)
      .then((layer) => {
        if (!cancelled) {
          setState({ status: "ready", layer });
          if (samePage) {
            // Character ranges from the previous content are no longer valid.
            setSelection(null);
          }
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setState({ status: "error", error: toCommandError(error) });
        }
      });

    return () => {
      cancelled = true;
    };
  }, [documentId, pageIndex, reloadKey]);

  useEffect(() => {
    let cancelled = false;
    editHistory(documentId)
      .then((next) => {
        if (!cancelled) {
          setHistory(next);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setHistory(null);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [documentId, historyRefreshKey]);

  useEffect(() => {
    if (!boxMode && !editMode) {
      setBoxes([]);
      setSelectedBoxId(null);
      return;
    }
    let cancelled = false;
    textBoxes(documentId, pageIndex)
      .then((list) => {
        if (!cancelled) {
          setBoxes(list);
          setSelectedBoxId((current) =>
            current !== null && list.some((box) => box.boxId === current)
              ? current
              : null,
          );
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setBoxes([]);
          setSelectedBoxId(null);
          setNotice({ kind: "error", text: toCommandError(error).message });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [boxMode, editMode, documentId, pageIndex, reloadKey]);

  useEffect(() => {
    if (!boxMode && !editMode) {
      return;
    }
    let cancelled = false;
    textBoxFonts()
      .then((list) => {
        if (!cancelled) {
          setFonts(list);
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setFonts([]);
          setNotice({ kind: "error", text: toCommandError(error).message });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [boxMode, editMode]);

  useEffect(
    () =>
      onDocumentChanged((detail) => {
        if (detail.documentId !== documentId) {
          return;
        }
        if (detail.pageIndex === pageIndex) {
          setReloadKey((key) => key + 1);
        }
        // The undo stack is document-wide: a form fill, an image edit, a shape
        // or another tool's change alters the availability of this toolbar's
        // undo/redo buttons. This layer's own commands already returned the
        // new history, so their notification is only for the page content.
        if (detail.source !== "text") {
          setHistoryRefreshKey((key) => key + 1);
        }
      }),
    [documentId, pageIndex],
  );

  useEffect(
    () => () => {
      targetRequest.current += 1;
    },
    [],
  );

  // Grow the editor with wrapped input so the user sees the same line breaks
  // the committed Text Box will use.
  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) {
      return;
    }
    textarea.style.height = "auto";
    textarea.style.height = `${textarea.scrollHeight}px`;
  }, [editor, boxMode, scale, state, editorFont]);

  // A selection belongs to one editor session: closing the editor or opening
  // another box must not carry its offsets into a different text.
  useEffect(() => {
    setEditorSelection(null);
  }, [editor?.status === "editing" ? editor.target.boxId : null]);
  useEffect(() => {
    if (
      selectNewParagraph.current &&
      editor?.status === "editing" &&
      textareaRef.current
    ) {
      selectNewParagraph.current = false;
      textareaRef.current.select();
      setEditorSelection({ start: 0, end: editor.draft.length });
    }
  }, [editor]);

  const layer = state.status === "ready" ? state.layer : null;
  const structure = useMemo(
    () => (layer ? buildTextLayer(layer.chars) : null),
    [layer],
  );
  const selectedBox =
    (boxMode || editMode) && selectedBoxId !== null
      ? (boxes.find((box) => box.boxId === selectedBoxId) ?? null)
      : null;
  const contentGuides = editMode || Boolean(workspaceTools && boxMode);
  const editBlocks = useMemo(() => {
    if (!contentGuides || !layer || layer.rotation) return [];
    let rects = editableTextBlocks(layer.chars);
    for (const box of boxes) {
      if (box.rotation) continue;
      rects = rects.filter(
        (rect) =>
          !(
            rect.x >= box.x - 1 &&
            rect.y >= box.y - 1 &&
            rect.x + rect.width <= box.x + box.width + 1 &&
            rect.y + rect.height <= box.y + box.height + 1
          ),
      );
      if (box.boxId !== selectedBoxId)
        rects.push({
          x: box.x,
          y: box.y,
          width: box.width,
          height: box.height,
        });
    }
    return rects;
  }, [contentGuides, layer, boxes, selectedBoxId]);
  const editorRange =
    editor?.status === "editing" && editorSelection
      ? codePointRange(editor.draft, editorSelection.start, editorSelection.end)
      : null;

  if (state.status === "loading") {
    return null;
  }

  if (state.status === "error") {
    return (
      <div
        className="page-text-layer page-text-layer--message"
        role="status"
        title={state.error.message}
      >
        <span className="text-layer-message">文字层无法载入</span>
      </div>
    );
  }

  const editingBusy = editor?.status === "editing" && editor.busy;
  const operationBusy =
    Boolean(editingBusy) || boxBusy || markupBusy || inkBusy || noteBusy;
  const annotationTools = canAnnotate && !readOnly;
  const toolbarFor = (canEdit: boolean, stylePanel: ReactNode) => (
    <TextEditToolbar
      canEdit={canEdit}
      canAnnotate={canAnnotate}
      editMode={editMode}
      boxMode={boxMode}
      noteMode={noteMode}
      inkMode={inkMode}
      inkColor={inkColor}
      inkOpacity={inkOpacity}
      inkLineWidth={inkLineWidth}
      readOnly={readOnly}
      busy={operationBusy}
      history={history}
      notice={notice}
      onToggleEdit={toggleEditMode}
      onToggleBoxMode={toggleBoxMode}
      onToggleNoteMode={toggleNoteMode}
      onToggleInkMode={toggleInkMode}
      onInkColor={setInkColor}
      onInkOpacity={setInkOpacity}
      onInkLineWidth={setInkLineWidth}
      onUndo={() => void runHistory("undo")}
      onRedo={() => void runHistory("redo")}
      stylePanel={stylePanel}
    />
  );

  if (!layer || !structure) {
    return null;
  }

  const textLayer: TextLayerView = layer;
  const textStructure: TextLayerStructure = structure;

  function cancelEditor() {
    targetRequest.current += 1;
    setEditor(null);
  }

  function toggleEditMode() {
    if (readOnly) {
      return;
    }
    if (editMode) {
      setEditMode(false);
      cancelEditor();
      return;
    }
    setEditMode(true);
    setBoxMode(false);
    setNoteMode(false);
    setInkMode(false);
    setInkDraft(null);
    setBoxInteraction(null);
    setSelection(null);
    setCopyStatus("idle");
    setNotice(null);
  }

  function toggleBoxMode() {
    if (readOnly) {
      return;
    }
    if (boxMode) {
      setBoxMode(false);
      setBoxInteraction(null);
      setSelectedBoxId(null);
      setEditor(null);
      return;
    }
    setBoxMode(true);
    setEditMode(false);
    setNoteMode(false);
    setInkMode(false);
    setInkDraft(null);
    setBoxInteraction(null);
    setSelection(null);
    setCopyStatus("idle");
    setNotice(null);
  }

  function toggleNoteMode() {
    if (!annotationTools) {
      return;
    }
    if (noteMode) {
      setNoteMode(false);
      return;
    }
    setNoteMode(true);
    setInkMode(false);
    setInkDraft(null);
    setEditMode(false);
    cancelEditor();
    setBoxMode(false);
    setBoxInteraction(null);
    setSelection(null);
    setCopyStatus("idle");
    setNotice(null);
  }

  function toggleInkMode() {
    if (!annotationTools) {
      return;
    }
    if (inkMode) {
      setInkMode(false);
      setInkDraft(null);
      return;
    }
    setInkMode(true);
    setNoteMode(false);
    setEditMode(false);
    cancelEditor();
    setBoxMode(false);
    setBoxInteraction(null);
    setSelection(null);
    setCopyStatus("idle");
    setNotice(null);
  }

  async function createBox(rect: BoxRect) {
    setBoxBusy(true);
    setNotice(null);
    try {
      const result = await createTextBox(documentId, pageIndex, {
        ...rect,
        ...(workspaceTools
          ? {
              style: {
                fontFamily: "Helvetica",
                fontSize: 12,
                bold: false,
                italic: false,
                underline: false,
                color: "#000000",
                alignment: "left" as const,
              },
            }
          : {}),
      });
      setHistory(result.history);
      setBoxes((current) => [
        ...current.filter((box) => box.boxId !== result.target.boxId),
        result.target,
      ]);
      setSelectedBoxId(result.target.boxId);
      selectNewParagraph.current = Boolean(workspaceTools);
      setEditor({
        status: "editing",
        target: result.target,
        draft: workspaceTools ? "在这里插入文字" : "",
        busy: false,
        error: null,
      });
      setNotice(fontFallbackNotice(result.font));
      notifyDocumentChanged({ documentId, pageIndex, source: "text" });
    } catch (error) {
      setNotice({ kind: "error", text: toCommandError(error).message });
    } finally {
      setBoxBusy(false);
    }
  }

  async function transformBox(
    box: TextBoxTargetView,
    rect: BoxRect,
    autoHeight = box.autoHeight,
    rotation = box.rotation,
    onCommitted?: () => void,
  ) {
    setBoxBusy(true);
    setNotice(null);
    try {
      const result = await transformTextBox(documentId, pageIndex, box.boxId, {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        autoHeight,
        rotation,
      });
      setHistory(result.history);
      setBoxes((current) =>
        current.map((item) =>
          item.boxId === result.target.boxId ? result.target : item,
        ),
      );
      onCommitted?.();
      notifyDocumentChanged({ documentId, pageIndex, source: "text" });
    } catch (error) {
      setPendingMove(null);
      setNotice({ kind: "error", text: toCommandError(error).message });
    } finally {
      setBoxBusy(false);
    }
  }

  async function applyBoxStyle(style: TextBoxStyleView) {
    const box = selectedBox;
    if (!box || boxBusy) {
      return;
    }
    setBoxBusy(true);
    setNotice(null);
    try {
      const result = await applyTextBoxStyle(
        documentId,
        pageIndex,
        box.boxId,
        style,
      );
      setHistory(result.history);
      setBoxes((current) =>
        current.map((item) =>
          item.boxId === result.target.boxId ? result.target : item,
        ),
      );
      setNotice(fontFallbackNotice(result.font));
      notifyDocumentChanged({ documentId, pageIndex, source: "text" });
    } catch (error) {
      setNotice({ kind: "error", text: toCommandError(error).message });
    } finally {
      setBoxBusy(false);
    }
  }

  // Styles the open editor: a selected range keeps its own run style while an
  // empty selection restyles the whole box. Both are one undoable action.
  async function applyEditorStyle(
    style: TextBoxStyleView,
    range: TextRangeView | null,
  ) {
    if (!editor || editor.status !== "editing" || editor.busy) {
      return;
    }
    const current = {...editor, draft: textareaRef.current?.value ?? editor.draft};
    const rawSelection = editorSelection;
    setEditor({ ...current, busy: true, error: null });
    setNotice(null);
    try {
      if (current.draft !== current.target.text) {
        const committed = await applyTextEdit(
          documentId,
          pageIndex,
          current.target.boxId,
          current.draft,
        );
        setHistory(committed.history);
        notifyDocumentChanged({ documentId, pageIndex, source: "text" });
      }
      const result = range
        ? await applyTextBoxSelectionStyle(
            documentId,
            pageIndex,
            current.target.boxId,
            range,
            style,
          )
        : await applyTextBoxStyle(
            documentId,
            pageIndex,
            current.target.boxId,
            style,
          );
      setHistory(result.history);
      setSelectedBoxId(result.target.boxId);
      setBoxes((currentBoxes) =>
        currentBoxes.map((item) =>
          item.boxId === result.target.boxId ? result.target : item,
        ),
      );
      setEditor((open) =>
        open?.status === "editing"
          ? { ...open, target: result.target, busy: false, error: null }
          : open,
      );
      if (range && rawSelection) {
        // A control may have taken focus; the text did not change, so the
        // textarea's offsets still address the characters just styled.
        textareaRef.current?.setSelectionRange(
          rawSelection.start,
          rawSelection.end,
        );
      }
      setNotice(fontFallbackNotice(result.font));
      notifyDocumentChanged({ documentId, pageIndex, source: "text" });
    } catch (error) {
      setEditor((open) =>
        open?.status === "editing"
          ? { ...open, busy: false, error: toCommandError(error).message }
          : open,
      );
    }
  }

  function trackEditorSelection(textarea: HTMLTextAreaElement) {
    setEditorSelection(
      textarea.selectionStart === textarea.selectionEnd
        ? null
        : { start: textarea.selectionStart, end: textarea.selectionEnd },
    );
  }

  async function deleteSelectedBox() {
    const box = selectedBox;
    if (!box || boxBusy) {
      return;
    }
    setBoxBusy(true);
    setNotice(null);
    try {
      const result = await deleteTextBox(documentId, pageIndex, box.boxId);
      setHistory(result.history);
      setBoxes((current) => current.filter((item) => item.boxId !== box.boxId));
      setSelectedBoxId(null);
      setEditor(null);
      notifyDocumentChanged({ documentId, pageIndex, source: "text" });
    } catch (error) {
      setNotice({ kind: "error", text: toCommandError(error).message });
    } finally {
      setBoxBusy(false);
    }
  }

  async function openEditor(charIndex: number) {
    if (readOnly) {
      return;
    }
    const requestId = targetRequest.current + 1;
    targetRequest.current = requestId;
    setEditor({ status: "loading" });
    setNotice(null);
    try {
      const target = await textEditTarget(documentId, pageIndex, charIndex);
      if (targetRequest.current === requestId) {
        setEditor({
          status: "editing",
          target,
          draft: target.text,
          busy: false,
          error: null,
        });
      }
    } catch (error) {
      if (targetRequest.current === requestId) {
        setEditor(null);
        setNotice({ kind: "error", text: toCommandError(error).message });
      }
    }
  }

  // Clicking an existing Text Object in Text Box mode turns it into a Text Box
  // that starts with the object's own geometry, font, size and colour, so the
  // first transform or style change edits the original text instead of
  // covering it with a new empty box.
  async function selectExistingBox(charIndex: number) {
    if (readOnly) {
      return;
    }
    enterEditingAfterResolve.current = false;
    const requestId = targetRequest.current + 1;
    targetRequest.current = requestId;
    setBoxBusy(true);
    setNotice(null);
    try {
      const target = await textEditTarget(documentId, pageIndex, charIndex);
      if (targetRequest.current === requestId) {
        setBoxes((current) => [
          ...current.filter((box) => box.boxId !== target.boxId),
          target,
        ]);
        setSelectedBoxId(target.boxId);
        setEditor(
          enterEditingAfterResolve.current
            ? {
                status: "editing",
                target,
                draft: target.text,
                busy: false,
                error: null,
              }
            : null,
        );
        enterEditingAfterResolve.current = false;
      }
    } catch (error) {
      if (targetRequest.current === requestId) {
        setNotice({ kind: "error", text: toCommandError(error).message });
      }
    } finally {
      if (targetRequest.current === requestId) {
        setBoxBusy(false);
      }
    }
  }

  function openBoxEditor(box: TextBoxTargetView) {
    if (readOnly || boxBusy) {
      return;
    }
    targetRequest.current += 1;
    setSelectedBoxId(box.boxId);
    setEditor({
      status: "editing",
      target: box,
      draft: box.text,
      busy: false,
      error: null,
    });
    setNotice(null);
  }

  async function commitEditor(): Promise<boolean> {
    if (!editor || editor.status !== "editing" || editor.busy) {
      return false;
    }
    const current = {...editor, draft: textareaRef.current?.value ?? editor.draft};
    if (current.draft === current.target.text) {
      setEditor(null);
      return true;
    }
    if (!current.draft.trim() && !current.target.created) {
      setEditor({
        ...current,
        error: "文字不能为空；如需移除文字，请使用删除。",
      });
      return false;
    }
    setEditor({ ...current, busy: true, error: null });
    try {
      const result = await applyTextEdit(
        documentId,
        pageIndex,
        current.target.boxId,
        current.draft,
      );
      setHistory(result.history);
      setEditor(null);
      if (boxMode) {
        setSelectedBoxId(current.target.boxId);
      }
      setNotice(fontFallbackNotice(result.font));
      notifyDocumentChanged({ documentId, pageIndex, source: "text" });
      return true;
    } catch (error) {
      setEditor({
        ...current,
        busy: false,
        error: toCommandError(error).message,
      });
      return false;
    }
  }

  pendingCommit.current = commitEditor;

  async function removeEditor() {
    if (!editor || editor.status !== "editing" || editor.busy) {
      return;
    }
    const current = editor;
    setEditor({ ...current, busy: true, error: null });
    try {
      const result = await deleteTextBox(
        documentId,
        pageIndex,
        current.target.boxId,
      );
      setHistory(result.history);
      setEditor(null);
      if (boxMode) {
        setSelectedBoxId(null);
      }
      setNotice(null);
      notifyDocumentChanged({ documentId, pageIndex, source: "text" });
    } catch (error) {
      setEditor({
        ...current,
        busy: false,
        error: toCommandError(error).message,
      });
    }
  }

  async function runHistory(kind: "undo" | "redo") {
    if (kind === "undo" ? !history?.canUndo : !history?.canRedo) {
      return;
    }
    try {
      const step =
        kind === "undo"
          ? await undoTextEdit(documentId)
          : await redoTextEdit(documentId);
      setHistory(step.history);
      setEditor(null);
      setSelection(null);
      setSelectedBoxId(null);
      setBoxInteraction(null);
      setInkDraft(null);
      setNoteInteraction(null);
      setNotice(null);
      notifyDocumentChanged({
        documentId,
        pageIndex: step.pageIndex,
        source: "text",
      });
    } catch (error) {
      setNotice({ kind: "error", text: toCommandError(error).message });
    }
  }

  const pointFromEvent = (event: {
    clientX: number;
    clientY: number;
  }): TextPoint | null => {
    const surface = surfaceRef.current;
    if (!surface || textLayer.pageWidth <= 0 || textLayer.pageHeight <= 0) {
      return null;
    }
    const box = surface.getBoundingClientRect();
    if (box.width <= 0 || box.height <= 0) {
      return null;
    }
    const x = ((event.clientX - box.left) / box.width) * textLayer.pageWidth;
    const y = ((event.clientY - box.top) / box.height) * textLayer.pageHeight;
    return {
      x: Math.min(Math.max(x, 0), textLayer.pageWidth),
      y: Math.min(Math.max(y, 0), textLayer.pageHeight),
    };
  };

  function clampInPage(value: number, maximum: number): number {
    return Math.min(Math.max(value, 0), Math.max(0, maximum));
  }

  function noteRectFrom(
    interaction: NoteInteraction,
    current: InkPoint,
  ): TextMarkupRect {
    const base = interaction.annotation.rects[0] ?? {
      x: 0,
      y: 0,
      width: DEFAULT_NOTE_SIZE,
      height: DEFAULT_NOTE_SIZE,
    };
    const dx = current.x - interaction.start.x;
    const dy = current.y - interaction.start.y;
    if (interaction.kind === "move") {
      return {
        x: clampInPage(base.x + dx, textLayer.pageWidth - base.width),
        y: clampInPage(base.y + dy, textLayer.pageHeight - base.height),
        width: base.width,
        height: base.height,
      };
    }
    const maxWidth = textLayer.pageWidth - base.x;
    const maxHeight = textLayer.pageHeight - base.y;
    return {
      x: base.x,
      y: base.y,
      width: Math.max(
        Math.min(MIN_NOTE_SIZE, maxWidth),
        Math.min(base.width + dx, maxWidth),
      ),
      height: Math.max(
        Math.min(MIN_NOTE_SIZE, maxHeight),
        Math.min(base.height + dy, maxHeight),
      ),
    };
  }

  function noteRectFor(annotation: AnnotationView): TextMarkupRect {
    if (
      noteInteraction !== null &&
      noteInteraction.annotation.name === annotation.name
    ) {
      return noteRectFrom(noteInteraction, noteInteraction.current);
    }
    return (
      annotation.rects[0] ?? {
        x: 0,
        y: 0,
        width: DEFAULT_NOTE_SIZE,
        height: DEFAULT_NOTE_SIZE,
      }
    );
  }

  async function createNoteAt(point: TextPoint) {
    if (!annotationTools || noteBusy) {
      return;
    }
    const width = Math.max(1, Math.min(DEFAULT_NOTE_SIZE, textLayer.pageWidth));
    const height = Math.max(
      1,
      Math.min(DEFAULT_NOTE_SIZE, textLayer.pageHeight),
    );
    const rect: TextMarkupRect = {
      x: clampInPage(point.x - width / 2, textLayer.pageWidth - width),
      y: clampInPage(point.y - height / 2, textLayer.pageHeight - height),
      width,
      height,
    };
    setNoteBusy(true);
    setNotice(null);
    try {
      const result = await createNoteAnnotation(documentId, {
        pageIndex,
        color: DEFAULT_NOTE_COLOR,
        opacity: 100,
        text: "",
        rect,
      });
      setHistory(result.history);
      setNoteMode(false);
      onSelectAnnotation?.(result.annotation);
      notifyDocumentChanged({ documentId, pageIndex });
    } catch (error) {
      setNotice({ kind: "error", text: toCommandError(error).message });
    } finally {
      setNoteBusy(false);
    }
  }

  async function createInk(stroke: InkPoint[]) {
    if (!annotationTools || inkBusy || stroke.length === 0) {
      return;
    }
    setInkBusy(true);
    setNotice(null);
    try {
      const result = await createInkAnnotation(documentId, {
        pageIndex,
        color: inkColor,
        opacity: inkOpacity,
        lineWidth: inkLineWidth,
        strokes: [stroke],
      });
      setHistory(result.history);
      onSelectAnnotation?.(result.annotation);
      notifyDocumentChanged({ documentId, pageIndex });
    } catch (error) {
      setNotice({ kind: "error", text: toCommandError(error).message });
    } finally {
      setInkBusy(false);
    }
  }

  async function commitNoteRect(
    annotation: AnnotationView,
    rect: TextMarkupRect,
  ) {
    setNoteBusy(true);
    setNotice(null);
    try {
      const result = await transformNoteAnnotation(
        documentId,
        annotation.name,
        rect,
      );
      setHistory(result.history);
      onSelectAnnotation?.(result.annotation);
      notifyDocumentChanged({ documentId, pageIndex });
    } catch (error) {
      setNotice({ kind: "error", text: toCommandError(error).message });
    } finally {
      setNoteBusy(false);
    }
  }

  function startNoteInteraction(
    event: ReactPointerEvent<HTMLElement>,
    annotation: AnnotationView,
    kind: "move" | "resize",
  ) {
    event.stopPropagation();
    event.preventDefault();
    if (!annotationTools || noteBusy) {
      return;
    }
    onSelectAnnotation?.(annotation);
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    setNoteInteraction({ kind, annotation, start: point, current: point });
    try {
      event.currentTarget.setPointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture is unavailable in some embedded webviews.
    }
  }

  function handleNotePointerMove(event: ReactPointerEvent<HTMLElement>) {
    if (!noteInteraction) {
      return;
    }
    event.stopPropagation();
    const point = pointFromEvent(event);
    if (point) {
      setNoteInteraction({ ...noteInteraction, current: point });
    }
  }

  function finishNoteInteraction(event: ReactPointerEvent<HTMLElement>) {
    if (!noteInteraction) {
      return;
    }
    event.stopPropagation();
    const interaction = noteInteraction;
    setNoteInteraction(null);
    try {
      event.currentTarget.releasePointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture may never have been acquired.
    }
    const point = pointFromEvent(event) ?? interaction.current;
    const rect = noteRectFrom(interaction, point);
    const original = interaction.annotation.rects[0];
    const changed =
      !original ||
      Math.abs(rect.x - original.x) > 0.01 ||
      Math.abs(rect.y - original.y) > 0.01 ||
      Math.abs(rect.width - original.width) > 0.01 ||
      Math.abs(rect.height - original.height) > 0.01;
    if (changed) {
      void commitNoteRect(interaction.annotation, rect);
    }
  }

  const selectAt = (
    point: TextPoint,
    kind: "character" | "word" | "line",
  ): TextRange | null => {
    const index = findCharAt(textLayer, point);
    if (index === null) {
      return null;
    }
    if (kind === "word") {
      return selectionForWord(textStructure, index) ?? selectionForChar(index);
    }
    if (kind === "line") {
      return selectionForLine(textStructure, index) ?? selectionForChar(index);
    }
    return selectionForChar(index);
  };

  const startBoxMove = (
    event: ReactPointerEvent<HTMLDivElement>,
    box: TextBoxTargetView,
  ) => {
    event.stopPropagation();
    if (readOnly || boxBusy || event.button !== 0) {
      return;
    }
    surfaceRef.current?.focus({ preventScroll: true });
    setSelectedBoxId(box.boxId);
    if (editor) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    setBoxInteraction({ kind: "move", box, start: point, current: point });
    try {
      // Capturing on the outline keeps pointer events bubbling to the layer,
      // while double-click still targets the outline so it can open the editor.
      event.currentTarget.setPointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture is unavailable in some embedded webviews.
    }
  };

  const startBoxResize = (
    event: ReactPointerEvent<HTMLButtonElement>,
    box: TextBoxTargetView,
    handle?: ResizeHandle,
  ) => {
    event.stopPropagation();
    event.preventDefault();
    if (readOnly || boxBusy) {
      return;
    }
    setSelectedBoxId(box.boxId);
    if (editor) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    setBoxInteraction({
      kind: "resize",
      box,
      handle,
      start: point,
      current: point,
    });
    try {
      event.currentTarget.setPointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture is unavailable in some embedded webviews.
    }
  };

  const startBoxRotate = (
    event: ReactPointerEvent<HTMLButtonElement>,
    box: TextBoxTargetView,
  ) => {
    event.stopPropagation();
    event.preventDefault();
    if (readOnly || boxBusy) {
      return;
    }
    setSelectedBoxId(box.boxId);
    if (editor) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    setBoxInteraction({ kind: "rotate", box, start: point, current: point });
    try {
      event.currentTarget.setPointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture is unavailable in some embedded webviews.
    }
  };

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }

    if (noteMode) {
      if (!annotationTools || noteBusy) {
        return;
      }
      event.preventDefault();
      surfaceRef.current?.focus({ preventScroll: true });
      void createNoteAt(point);
      return;
    }

    if (inkMode) {
      if (!annotationTools || inkBusy) {
        return;
      }
      event.preventDefault();
      surfaceRef.current?.focus({ preventScroll: true });
      setNotice(null);
      setInkDraft([point]);
      try {
        event.currentTarget.setPointerCapture?.(event.pointerId);
      } catch {
        // Pointer capture is unavailable in some embedded webviews.
      }
      return;
    }

    if (boxMode) {
      if (editor?.status === "editing" && workspaceTools) {
        event.preventDefault();
        void commitEditor().then((saved) => {
          if (saved) setSelectedBoxId(null);
        });
        return;
      }
      if (editor || boxBusy) return;
      event.preventDefault();
      surfaceRef.current?.focus({ preventScroll: true });
      const index = findCharAt(textLayer, point);
      if (index !== null) {
        void selectExistingBox(index);
        return;
      }
      setBoxInteraction({ kind: "create", anchor: point, current: point });
      try {
        surfaceRef.current?.setPointerCapture?.(event.pointerId);
      } catch {
        // Pointer capture is unavailable in some embedded webviews.
      }
      return;
    }

    if (editMode) {
      event.preventDefault();
      if (boxBusy) return;
      const index = findCharAt(textLayer, point);
      const select = () => {
        if (index !== null) void selectExistingBox(index);
        else {
          setSelectedBoxId(null);
          setEditor(null);
        }
      };
      if (editor?.status === "editing") {
        // Read the live composing value before moving focus. WKWebView can
        // cancel the pending IME suffix synchronously when the textarea blurs.
        void commitEditor().then((saved) => {
          if (saved) {
            surfaceRef.current?.focus({ preventScroll: true });
            select();
          }
        });
      } else {
        surfaceRef.current?.focus({ preventScroll: true });
        select();
      }
      return;
    }

    event.preventDefault();
    surfaceRef.current?.focus({ preventScroll: true });
    setCopyStatus("idle");

    if (event.detail >= 2) {
      // Double and triple clicks are resolved by dblclick/click so the
      // selected word or line is not immediately replaced by a character.
      dragAnchor.current = null;
      return;
    }

    const index = findCharAt(textLayer, point);
    if (index === null) {
      dragAnchor.current = null;
      setSelection(null);
      return;
    }

    dragAnchor.current = index;
    setSelection(selectionForChar(index));
    try {
      event.currentTarget.setPointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture is unavailable in some embedded webviews.
    }
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const boxInteraction = interactionRef.current;
    if (inkDraft) {
      const point = pointFromEvent(event);
      if (point) {
        const last = inkDraft[inkDraft.length - 1];
        if (Math.hypot(point.x - last.x, point.y - last.y) >= 0.75) {
          setInkDraft([...inkDraft, point]);
        }
      }
      return;
    }
    if (boxInteraction?.kind === "create") {
      const point = pointFromEvent(event);
      if (point) {
        queueBoxInteraction({ ...boxInteraction, current: point });
      }
      return;
    }
    if (boxInteraction?.kind === "move") {
      const point = pointFromEvent(event);
      if (point) {
        queueBoxInteraction({ ...boxInteraction, current: point });
      }
      return;
    }
    if (boxInteraction?.kind === "resize") {
      const point = pointFromEvent(event);
      if (point) {
        queueBoxInteraction({ ...boxInteraction, current: point });
      }
      return;
    }
    if (boxInteraction?.kind === "rotate") {
      const point = pointFromEvent(event);
      if (point) {
        queueBoxInteraction({ ...boxInteraction, current: point });
      }
      return;
    }
    if (editMode || boxMode || dragAnchor.current === null) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    const focus = findCharAt(textLayer, point, 24);
    if (focus !== null) {
      setSelection(selectionForDrag(dragAnchor.current, focus));
    }
  };

  const endDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    const boxInteraction = interactionRef.current;
    if (inkDraft) {
      const stroke = [...inkDraft];
      const point = pointFromEvent(event);
      if (point) {
        const last = stroke[stroke.length - 1];
        if (Math.hypot(point.x - last.x, point.y - last.y) >= 0.5) {
          stroke.push(point);
        }
      }
      setInkDraft(null);
      try {
        event.currentTarget.releasePointerCapture?.(event.pointerId);
      } catch {
        // Pointer capture may never have been acquired.
      }
      void createInk(stroke);
      return;
    }
    if (boxInteraction?.kind === "rotate") {
      const interaction = boxInteraction;
      setBoxInteraction(null);
      try {
        event.currentTarget.releasePointerCapture?.(event.pointerId);
      } catch {
        // Pointer capture may never have been acquired.
      }
      const point = pointFromEvent(event) ?? interaction.current;
      const rotation = rotationFromPointer(
        interaction.box,
        point,
        event.shiftKey ? 15 : 0,
      );
      if (rotation !== interaction.box.rotation) {
        void transformBox(
          interaction.box,
          interaction.box,
          interaction.box.autoHeight,
          rotation,
        );
      }
      return;
    }
    if (boxInteraction?.kind === "resize") {
      const interaction = boxInteraction;
      setBoxInteraction(null);
      try {
        event.currentTarget.releasePointerCapture?.(event.pointerId);
      } catch {
        // Pointer capture may never have been acquired.
      }
      const point = pointFromEvent(event) ?? interaction.current;
      if (
        Math.hypot(
          point.x - interaction.start.x,
          point.y - interaction.start.y,
        ) <
        1 / scale
      )
        return;
      const resized = interaction.handle
        ? resizeBoxFromDrag(
            interaction.box,
            interaction.box.rotation,
            interaction.start,
            point,
            interaction.handle,
          )
        : resizeBox(interaction.box, interaction.box.rotation, point);
      if (
        (interaction.handle === "e" || interaction.handle === "w") &&
        Math.abs(resized.width - interaction.box.width) > 0.01
      )
        resized.height = interaction.box.fontSize * 1.2;
      if (
        resized.width !== interaction.box.width ||
        resized.height !== interaction.box.height
      ) {
        void transformBox(
          interaction.box,
          resized,
          Boolean(interaction.handle),
        );
      }
      return;
    }
    if (boxInteraction?.kind === "move") {
      const interaction = boxInteraction;
      setBoxInteraction(null);
      try {
        event.currentTarget.releasePointerCapture?.(event.pointerId);
      } catch {
        // Pointer capture may never have been acquired.
      }
      const point = pointFromEvent(event) ?? interaction.current;
      const moved = moveBox(
        interaction.box,
        {
          x: point.x - interaction.start.x,
          y: point.y - interaction.start.y,
        },
        textLayer.pageWidth,
        textLayer.pageHeight,
      );
      if (moved.x !== interaction.box.x || moved.y !== interaction.box.y) {
        if (moveCache.data) {
          setPendingMove({boxId: interaction.box.boxId, data: moveCache.data, rect: moved, origin: interaction.box, afterPaint: null});
        }
        void transformBox(interaction.box, moved, interaction.box.autoHeight, interaction.box.rotation, () => {
          if (paintVersionRef.current === undefined) setPendingMove(null);
          else setPendingMove(current => current ? {...current, afterPaint: paintVersionRef.current!} : null);
        });
      }
      return;
    }
    if (boxInteraction?.kind === "create") {
      const current = boxInteraction;
      setBoxInteraction(null);
      try {
        event.currentTarget.releasePointerCapture?.(event.pointerId);
      } catch {
        // Pointer capture may never have been acquired.
      }
      const point = pointFromEvent(event) ?? current.current;
      const placed = dragRect(current.anchor, point);
      if (
        workspaceTools &&
        (Math.abs(point.x - current.anchor.x) < 12 ||
          Math.abs(point.y - current.anchor.y) < 12)
      )
        placed.width = 120;
      const rect = fitBoxToPage(
        placed,
        textLayer.pageWidth,
        textLayer.pageHeight,
      );
      void createBox(rect);
      return;
    }
    dragAnchor.current = null;
    try {
      event.currentTarget.releasePointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture may never have been acquired.
    }
  };

  const cancelBoxInteraction = (event: ReactPointerEvent<HTMLDivElement>) => {
    setBoxInteraction(null);
    setInkDraft(null);
    setNoteInteraction(null);
    dragAnchor.current = null;
    try {
      event.currentTarget.releasePointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture may never have been acquired.
    }
  };

  const handleDoubleClick = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (editMode) {
      event.preventDefault();
      if (selectedBox) openBoxEditor(selectedBox);
      else if (boxBusy) enterEditingAfterResolve.current = true;
      else {
        const point = pointFromEvent(event);
        const index = point ? findCharAt(textLayer, point) : null;
        if (index !== null) void openEditor(index);
      }
      return;
    }
    if (boxMode || noteMode || inkMode) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    setCopyStatus("idle");
    setSelection(selectAt(point, "word"));
  };

  const handleClick = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (editMode || boxMode || noteMode || inkMode || event.detail !== 3) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    setCopyStatus("idle");
    setSelection(selectAt(point, "line"));
  };

  const copySelection = async (): Promise<void> => {
    if (!selection) {
      return;
    }
    const text = selectionText(textLayer, selection);
    if (!text) {
      return;
    }
    const clipboard = navigator.clipboard;
    if (!clipboard) {
      setCopyStatus("failed");
      return;
    }
    try {
      await clipboard.writeText(text);
      setCopyStatus("copied");
    } catch {
      setCopyStatus("failed");
    }
  };

  const createMarkup = async (kind: TextMarkupKind): Promise<void> => {
    if (!selection || markupBusy || readOnly || !canAnnotate) {
      return;
    }
    const rects = selectionRects(textStructure, selection);
    if (rects.length === 0) {
      return;
    }
    setMarkupBusy(true);
    setNotice(null);
    try {
      const result = await createTextMarkupAnnotation(documentId, {
        pageIndex,
        kind,
        color: markupColor,
        opacity: markupOpacity,
        rects,
      });
      setHistory(result.history);
      // The native annotation is now part of the page bitmap; clear the
      // selection so a second click cannot create a duplicate by accident.
      setSelection(null);
      setCopyStatus("idle");
      notifyDocumentChanged({ documentId, pageIndex });
    } catch (error) {
      setNotice({ kind: "error", text: toCommandError(error).message });
    } finally {
      setMarkupBusy(false);
    }
  };

  const openLinkDialog = (): void => {
    if (!selection || markupBusy || linkBusy || readOnly || !canAnnotate) {
      return;
    }
    const rects = selectionRects(textStructure, selection);
    if (rects.length === 0) {
      return;
    }
    setLinkError(null);
    setLinkDraft({ rects });
  };

  const createLinkFromSelection = async (target: LinkTarget): Promise<void> => {
    if (!linkDraft || linkBusy) {
      return;
    }
    setLinkBusy(true);
    setLinkError(null);
    try {
      const result = await createLink(documentId, {
        pageIndex,
        rects: linkDraft.rects,
        target,
      });
      setHistory(result.history);
      setLinkDraft(null);
      // The link now covers the selection; clear it so the action bar cannot
      // start a second link over the same text by accident.
      setSelection(null);
      setCopyStatus("idle");
      notifyDocumentChanged({ documentId, pageIndex, source: "link" });
    } catch (error) {
      setLinkError(linkErrorText(error));
    } finally {
      setLinkBusy(false);
    }
  };

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (
      event.target instanceof HTMLElement &&
      event.target.closest("button,input,select,textarea,[contenteditable]")
    )
      return;
    if ((editMode || workspaceTools) && selectedBox && !editor && !boxBusy) {
      if (event.key === "Delete" || event.key === "Backspace") {
        event.preventDefault();
        void deleteSelectedBox();
        return;
      }
      if (event.key === "Enter" || event.key === "F2") {
        event.preventDefault();
        openBoxEditor(selectedBox);
        return;
      }
      if (event.key.startsWith("Arrow")) {
        const step = event.shiftKey ? 10 : 1;
        const delta = {
          x:
            event.key === "ArrowLeft"
              ? -step
              : event.key === "ArrowRight"
                ? step
                : 0,
          y:
            event.key === "ArrowUp"
              ? -step
              : event.key === "ArrowDown"
                ? step
                : 0,
        };
        event.preventDefault();
        void transformBox(
          selectedBox,
          moveBox(
            selectedBox,
            delta,
            textLayer.pageWidth,
            textLayer.pageHeight,
          ),
        );
        return;
      }
    }
    if (event.key === "Escape") {
      setSelectedBoxId(null);
      if (editor) {
        event.preventDefault();
        cancelEditor();
        return;
      }
      if (inkDraft) {
        setInkDraft(null);
      }
      setSelection(null);
      setCopyStatus("idle");
      return;
    }
    if (editor) {
      // The textarea owns typing keys, including its own undo while composing.
      return;
    }
    // Undo/redo and page shortcuts belong to the native menu. The layer must
    // not also handle Cmd/Ctrl+Z here or one press would produce two history
    // steps; the textarea above keeps the platform's native text undo.
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "c") {
      event.preventDefault();
      void copySelection();
    }
  };

  const highlights = selection ? selectionRects(textStructure, selection) : [];
  const pageNotes = annotations.filter(
    (annotation) =>
      annotation.pageIndex === pageIndex && annotation.kind === "note",
  );
  const selectedCharacterCount = selection
    ? [...selectionText(textLayer, selection)].length
    : 0;
  const createPreview =
    boxInteraction?.kind === "create"
      ? fitBoxToPage(
          dragRect(boxInteraction.anchor, boxInteraction.current),
          textLayer.pageWidth,
          textLayer.pageHeight,
        )
      : null;
  const editorMinHeight =
    editor?.status === "editing"
      ? Math.max(
          editor.target.height,
          editor.target.autoHeight
            ? estimatedContentHeight(
                editor.draft,
                editor.target.width,
                editor.target.fontSize,
                editor.target.fontFamily,
              )
            : 0,
          editor.target.fontSize * 1.2,
        )
      : 0;
  // The floating action bar sits above the editor like iLovePDF's selection
  // toolbar, except at the top of a page where the layer would clip it.
  const editorActionsBelow =
    editor?.status === "editing" && editor.target.y * scale < 52;
  const previewRotationFor = (box: TextBoxTargetView): number => {
    if (
      boxInteraction?.kind === "rotate" &&
      boxInteraction.box.boxId === box.boxId
    ) {
      return rotationFromPointer(box, boxInteraction.current);
    }
    return box.rotation;
  };
  const previewRectFor = (box: TextBoxTargetView): BoxRect => {
    if (pendingMove?.boxId === box.boxId) return pendingMove.rect;
    if (
      boxInteraction?.kind === "move" &&
      boxInteraction.box.boxId === box.boxId
    ) {
      return moveBox(
        box,
        {
          x: boxInteraction.current.x - boxInteraction.start.x,
          y: boxInteraction.current.y - boxInteraction.start.y,
        },
        textLayer.pageWidth,
        textLayer.pageHeight,
      );
    }
    if (
      boxInteraction?.kind === "resize" &&
      boxInteraction.box.boxId === box.boxId
    ) {
      const rect = boxInteraction.handle
        ? resizeBoxFromDrag(
            box,
            box.rotation,
            boxInteraction.start,
            boxInteraction.current,
            boxInteraction.handle,
          )
        : resizeBox(box, box.rotation, boxInteraction.current);
      if (dragPreview?.target.boxId === box.boxId && Math.abs(dragPreview.target.width - rect.width) < 0.01) {
        return {...rect, height: dragPreview.target.height};
      }
      return rect;
    }
    return box;
  };
  const moving = boxInteraction?.kind === "move" && moveCache.data
    ? {data: moveCache.data, rect: previewRectFor(boxInteraction.box), origin: boxInteraction.box}
    : pendingMove;

  return (
    <div
      ref={surfaceRef}
      className={`page-text-layer${editMode || (workspaceTools && boxMode) ? " page-text-layer--editing" : ""}${
        boxMode ? " page-text-layer--box-mode" : ""
      }${noteMode ? " page-text-layer--note-mode" : ""}${
        inkMode ? " page-text-layer--ink-mode" : ""
      }${!textLayer.hasText ? " page-text-layer--message" : ""}${
        !textLayer.hasText && annotationTools
          ? " page-text-layer--annotation-tools"
          : ""
      }`}
      role={textLayer.hasText || annotationTools ? "region" : undefined}
      aria-label={textLayer.hasText || annotationTools ? "文本层" : undefined}
      tabIndex={0}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={endDrag}
      onPointerCancel={cancelBoxInteraction}
      onDoubleClick={handleDoubleClick}
      onClick={handleClick}
      onKeyDown={handleKeyDown}
    >
      {moving && (
        <>
          <img className="content-drag-preview" src={`data:image/png;base64,${moving.data.background.pngBase64}`} alt="" draggable={false} />
          <img className="content-move-sprite" data-testid="text-move-sprite"
            src={`data:image/png;base64,${moving.data.foreground.pngBase64}`} alt="" draggable={false}
            style={{left: moving.data.foreground.x / moving.data.foreground.scale * scale,
              top: moving.data.foreground.y / moving.data.foreground.scale * scale,
              width: moving.data.foreground.width / moving.data.foreground.scale * scale,
              height: moving.data.foreground.height / moving.data.foreground.scale * scale,
              transform: `translate3d(${(moving.rect.x-moving.origin.x)*scale}px, ${(moving.rect.y-moving.origin.y)*scale}px, 0)`}} />
        </>
      )}
      {previewing && dragPreview && (
        <img
          className="content-drag-preview"
          src={`data:image/png;base64,${dragPreview.image.pngBase64}`}
          alt=""
          draggable={false}
        />
      )}
      {toolbarFor(
        textLayer.hasText,
        editor?.status === "editing" ? (
          <TextBoxStylePanel
            target={editor.target}
            fonts={fonts}
            busy={editor.busy}
            editing
            selection={editorRange}
            onChange={(style, range) => void applyEditorStyle(style, range)}
            onDelete={() => void removeEditor()}
            onLink={() =>
              setLinkDraft({
                rects: [
                  {
                    x: editor.target.x,
                    y: editor.target.y,
                    width: editor.target.width,
                    height: editor.target.height,
                  },
                ],
              })
            }
          />
        ) : selectedBox ? (
          <TextBoxStylePanel
            target={selectedBox}
            showDelete={!workspaceTools}
            fonts={fonts}
            busy={boxBusy}
            onChange={(style) => void applyBoxStyle(style)}
            onDelete={() => void deleteSelectedBox()}
          />
        ) : null,
      )}

      {!textLayer.hasText && (
        <span className="text-layer-message" role="status">
          {SCANNED_REASON}
        </span>
      )}

      {editBlocks.map((rect, index) => {
        const rectRight = rect.x + rect.width;
        const rectBottom = rect.y + rect.height;
        const active =
          editor?.status === "editing" &&
          editor.target.x < rectRight &&
          editor.target.x + editor.target.width > rect.x &&
          editor.target.y < rectBottom &&
          editor.target.y + editor.target.height > rect.y;
        return (
          <div
            key={`edit-block:${index}`}
            className={`text-edit-block${active ? " text-edit-block--active" : ""}`}
            data-testid="text-edit-block"
            aria-hidden="true"
            style={{
              left: `${(rect.x / textLayer.pageWidth) * 100}%`,
              top: `${(rect.y / textLayer.pageHeight) * 100}%`,
              width: `${(rect.width / textLayer.pageWidth) * 100}%`,
              height: `${(rect.height / textLayer.pageHeight) * 100}%`,
            }}
          />
        );
      })}

      {highlights.map((rect, index) => (
        <div
          key={`${index}:${rect.x}:${rect.y}`}
          className="text-highlight"
          data-testid="text-highlight"
          aria-hidden="true"
          style={{
            left: `${(rect.x / textLayer.pageWidth) * 100}%`,
            top: `${(rect.y / textLayer.pageHeight) * 100}%`,
            width: `${(rect.width / textLayer.pageWidth) * 100}%`,
            height: `${(rect.height / textLayer.pageHeight) * 100}%`,
          }}
        />
      ))}

      {selectedAnnotation?.pageIndex === pageIndex &&
        selectedAnnotation.kind !== "note" &&
        selectedAnnotation.kind !== "stamp" &&
        selectedAnnotation.rects.map((rect, index) => (
          <div
            key={`selected:${index}:${rect.x}:${rect.y}`}
            className="annotation-selection-outline"
            data-testid="selected-annotation-outline"
            aria-hidden="true"
            style={{
              left: `${(rect.x / textLayer.pageWidth) * 100}%`,
              top: `${(rect.y / textLayer.pageHeight) * 100}%`,
              width: `${(rect.width / textLayer.pageWidth) * 100}%`,
              height: `${(rect.height / textLayer.pageHeight) * 100}%`,
              borderColor: selectedAnnotation.color,
            }}
          />
        ))}

      {pageNotes.map((annotation) => {
        const rect = noteRectFor(annotation);
        const selected = selectedAnnotation?.name === annotation.name;
        return (
          <div
            key={`note:${annotation.name}`}
            className={`annotation-note${selected ? " annotation-note--selected" : ""}`}
            role="button"
            tabIndex={0}
            aria-label={`便签：${annotation.text?.trim() || "空白"}`}
            aria-pressed={selected}
            data-testid="note-annotation"
            title={annotation.text?.trim() || "空白便签"}
            style={{
              left: `${(rect.x / textLayer.pageWidth) * 100}%`,
              top: `${(rect.y / textLayer.pageHeight) * 100}%`,
              width: `${(rect.width / textLayer.pageWidth) * 100}%`,
              height: `${(rect.height / textLayer.pageHeight) * 100}%`,
              borderColor: selected ? annotation.color : "transparent",
            }}
            onPointerDown={(event) =>
              startNoteInteraction(event, annotation, "move")
            }
            onPointerMove={handleNotePointerMove}
            onPointerUp={finishNoteInteraction}
            onPointerCancel={finishNoteInteraction}
            onClick={(event) => {
              event.stopPropagation();
              onSelectAnnotation?.(annotation);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                onSelectAnnotation?.(annotation);
              }
            }}
          >
            <span
              className="annotation-note-mark"
              aria-hidden="true"
              style={{
                backgroundColor: annotation.color,
                opacity: annotation.opacity / 100,
              }}
            />
            {selected && (
              <button
                type="button"
                className="annotation-note-handle"
                aria-label="调整便签大小"
                data-testid="note-resize-handle"
                onPointerDown={(event) =>
                  startNoteInteraction(event, annotation, "resize")
                }
                onPointerMove={handleNotePointerMove}
                onPointerUp={finishNoteInteraction}
                onPointerCancel={finishNoteInteraction}
              />
            )}
          </div>
        );
      })}

      {inkDraft && inkDraft.length > 0 && (
        <svg
          className="annotation-ink-draft"
          data-testid="ink-draft"
          aria-hidden="true"
          viewBox={`0 0 ${textLayer.pageWidth} ${textLayer.pageHeight}`}
          preserveAspectRatio="none"
        >
          {inkDraft.length === 1 ? (
            <circle
              cx={inkDraft[0].x}
              cy={inkDraft[0].y}
              r={inkLineWidth / 2}
              fill={inkColor}
              fillOpacity={inkOpacity / 100}
            />
          ) : (
            <polyline
              points={inkDraft
                .map((point) => `${point.x},${point.y}`)
                .join(" ")}
              fill="none"
              stroke={inkColor}
              strokeOpacity={inkOpacity / 100}
              strokeWidth={inkLineWidth}
              strokeLinecap="round"
              strokeLinejoin="round"
              vectorEffect="non-scaling-stroke"
            />
          )}
        </svg>
      )}

      {(boxMode || editMode) && (!editor || (!workspaceTools && boxMode)) && (
        <div className="text-box-overlay">
          {boxes
            .filter(
              (box) =>
                (boxMode && !workspaceTools) || box.boxId === selectedBoxId,
            )
            .map((box, index) => {
              const rect = previewRectFor(box);
              const frame = editMode || workspaceTools ? contentSelectionFrame(rect,previewRotationFor(box),scale) : rect;
              return (
                <div
                  key={box.boxId}
                  className={`text-box-outline${
                    selectedBoxId === box.boxId
                      ? " text-box-outline--selected"
                      : ""
                  }`}
                  role="group"
                  aria-label={
                    editMode || workspaceTools
                      ? "选中文本框"
                      : `文本框 ${index + 1}`
                  }
                  tabIndex={0}
                  data-testid={`text-box-outline-${box.boxId}`}
                  style={{
                    left: `${(frame.x / textLayer.pageWidth) * 100}%`,
                    top: `${(frame.y / textLayer.pageHeight) * 100}%`,
                    width: `${(frame.width / textLayer.pageWidth) * 100}%`,
                    height: `${(frame.height / textLayer.pageHeight) * 100}%`,
                    transform: `rotate(${previewRotationFor(box)}deg)`,
                  }}
                  onPointerDown={(event) => startBoxMove(event, box)}
                  onDoubleClick={(event) => {
                    event.stopPropagation();
                    openBoxEditor(box);
                  }}
                >
                  {!editMode && !workspaceTools && (
                    <span className="text-box-outline-label">
                      {box.text ? box.text.split("\n")[0] : "空文本框"}
                    </span>
                  )}
                  {selectedBoxId === box.boxId && (
                    <>
                      {editMode || workspaceTools ? (
                        <>
                          {RESIZE_HANDLES.map((handle) => (
                            <button
                              key={handle}
                              type="button"
                              className={`text-box-handle content-resize-handle content-resize-handle--${handle}`}
                              style={{width:12,height:12}}
                              aria-label={`调整文本框 ${handle}`}
                              data-testid={`text-box-resize-${handle}`}
                              onPointerDown={(event) =>
                                startBoxResize(event, box, handle)
                              }
                              onDoubleClick={(event) => event.stopPropagation()}
                            />
                          ))}
                          <div
                            className={`content-selection-actions${rect.y + rect.height + 52 / scale > textLayer.pageHeight ? " content-selection-actions--above" : ""}`}
                            role="toolbar"
                            aria-label="所选文字操作"
                            onPointerDown={(event) => event.stopPropagation()}
                          >
                            <button
                              type="button"
                              aria-label="编辑选中文字"
                              title="编辑"
                              onClick={() => openBoxEditor(box)}
                            >
                              <WorkspaceIcon name="editObject" />
                            </button>
                            <button
                              type="button"
                              aria-label="删除选中文字"
                              title="删除"
                              onClick={() => void deleteSelectedBox()}
                            >
                              <WorkspaceIcon name="trash" />
                            </button>
                          </div>
                        </>
                      ) : (
                        <>
                          <button
                            type="button"
                            className="text-box-handle text-box-handle--rotate"
                            aria-label="旋转"
                            data-testid="text-box-rotate-handle"
                            onPointerDown={(event) =>
                              startBoxRotate(event, box)
                            }
                            onDoubleClick={(event) => event.stopPropagation()}
                          />
                          <button
                            type="button"
                            className="text-box-handle text-box-handle--resize"
                            aria-label="调整大小"
                            data-testid="text-box-resize-handle"
                            onPointerDown={(event) =>
                              startBoxResize(event, box)
                            }
                            onDoubleClick={(event) => event.stopPropagation()}
                          />
                        </>
                      )}
                    </>
                  )}
                </div>
              );
            })}
        </div>
      )}

      {createPreview && (
        <div
          className="text-box-create-preview"
          data-testid="text-box-create-preview"
          aria-hidden="true"
          style={{
            left: `${(createPreview.x / textLayer.pageWidth) * 100}%`,
            top: `${(createPreview.y / textLayer.pageHeight) * 100}%`,
            width: `${(createPreview.width / textLayer.pageWidth) * 100}%`,
            height: `${(createPreview.height / textLayer.pageHeight) * 100}%`,
          }}
        />
      )}

      {editor?.status === "editing" && (
        <div
          className="text-box-editor"
          data-testid="text-box-editor"
          onPointerDown={(event) => event.stopPropagation()}
          style={{
            left: `${(editor.target.x / textLayer.pageWidth) * 100}%`,
            top: `${(editor.target.y / textLayer.pageHeight) * 100}%`,
            width: `${(editor.target.width / textLayer.pageWidth) * 100}%`,
            minHeight: `${editorMinHeight * scale}px`,
          }}
        >
          <textarea
            ref={textareaRef}
            className="text-box-input"
            aria-label="文字内容"
            value={editor.draft}
            rows={1}
            wrap="soft"
            autoFocus
            spellCheck={false}
            readOnly={editor.busy}
            onCompositionEnd={(event) => {const draft=event.currentTarget.value;setEditor(current => current?.status === "editing" ? {...current,draft,error:null} : current);}}
            style={{
              fontFamily: editorFont,
              // Same subpixel allowance as the native wrap calculation.
              width: `calc(100% + ${0.25 * scale}px)`,
              wordBreak: "keep-all",
              overflowWrap: "anywhere",
              fontSize: `${editor.target.fontSize * scale}px`,
              fontWeight: editor.target.bold ? 700 : 400,
              fontStyle: editor.target.italic ? "italic" : "normal",
              textDecoration:
                [
                  editor.target.underline ? "underline" : "",
                  editor.target.strikeout ? "line-through" : "",
                ]
                  .filter(Boolean)
                  .join(" ") || "none",
              whiteSpace: "pre-wrap",
              lineHeight: `${(editor.target.lineHeight ?? editor.target.fontSize * 1.2) * scale}px`,
              color: editor.target.color,
            }}
            onChange={(event) => {
              const draft = event.currentTarget.value;
              setEditor((current) =>
                current?.status === "editing"
                  ? {
                      ...current,
                      draft,
                      error: null,
                    }
                  : current,
              );
              trackEditorSelection(event.currentTarget);
            }}
            onSelect={(event) => trackEditorSelection(event.currentTarget)}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.preventDefault();
                event.stopPropagation();
                if (!workspaceTools) cancelEditor();
              } else if (
                (event.metaKey || event.ctrlKey) &&
                event.key === "Enter"
              ) {
                event.preventDefault();
                void commitEditor();
              }
            }}
          />
          {!workspaceTools && (
            <div
              className={`text-box-actions${editorActionsBelow ? " text-box-actions--below" : ""}`}
              role="toolbar"
              aria-label="文字编辑"
            >
              <button
                type="button"
                className="text-box-action text-box-action--apply"
                aria-label="应用"
                title="应用"
                disabled={editor.busy}
                onClick={() => void commitEditor()}
              >
                <WorkspaceIcon name="check" />
              </button>
              <button
                type="button"
                className="text-box-action"
                aria-label="取消"
                title="取消"
                disabled={editor.busy}
                onClick={cancelEditor}
              >
                <WorkspaceIcon name="close" />
              </button>
              <button
                type="button"
                className="text-box-action text-box-action--delete"
                aria-label="删除"
                title="删除"
                disabled={editor.busy}
                onClick={() => void removeEditor()}
              >
                <WorkspaceIcon name="trash" />
              </button>
            </div>
          )}
          {editor.error && (
            <span role="alert" className="text-box-error">
              {editor.error}
            </span>
          )}
        </div>
      )}

      {selection && selectedCharacterCount > 0 && (
        <div
          className="text-selection-actions"
          onPointerDown={(event) => event.stopPropagation()}
        >
          <span className="text-selection-count">
            已选择 {selectedCharacterCount} 个字符
          </span>
          <button type="button" onClick={() => void copySelection()}>
            复制
          </button>
          {copyStatus !== "idle" && (
            <span role="status" className="text-copy-status">
              {copyStatus === "copied" ? "已复制" : "复制失败"}
            </span>
          )}
          {canAnnotate && !readOnly ? (
            <span className="text-markup-controls">
              <span className="toolbar-divider" aria-hidden="true" />
              <label className="text-markup-control">
                颜色
                <input
                  type="color"
                  aria-label="注释颜色"
                  value={markupColor}
                  disabled={markupBusy}
                  onChange={(event) => setMarkupColor(event.target.value)}
                />
              </label>
              <label className="text-markup-control">
                透明度
                <input
                  type="range"
                  aria-label="注释透明度"
                  min={0}
                  max={100}
                  step={1}
                  value={markupOpacity}
                  disabled={markupBusy}
                  onChange={(event) =>
                    setMarkupOpacity(Number.parseInt(event.target.value, 10))
                  }
                />
                <span>{markupOpacity}%</span>
              </label>
              <button
                type="button"
                disabled={markupBusy}
                onClick={() => void createMarkup("highlight")}
              >
                高亮
              </button>
              <button
                type="button"
                disabled={markupBusy}
                onClick={() => void createMarkup("underline")}
              >
                下划线
              </button>
              <button
                type="button"
                disabled={markupBusy}
                onClick={() => void createMarkup("strikeout")}
              >
                删除线
              </button>
              <button
                type="button"
                disabled={markupBusy}
                onClick={() => void createMarkup("squiggly")}
              >
                波浪线
              </button>
              <span className="toolbar-divider" aria-hidden="true" />
              <button
                type="button"
                disabled={markupBusy || linkBusy}
                title="为此段文字添加超链接"
                onClick={openLinkDialog}
              >
                链接
              </button>
            </span>
          ) : (
            <span className="text-annotation-reason">
              {ANNOTATION_PERMISSION_REASON}
            </span>
          )}
        </div>
      )}

      {linkDraft &&
        createPortal(
          <LinkTargetDialog
            mode="create"
            pageCount={pageCount}
            initialTarget={null}
            busy={linkBusy}
            error={linkError}
            onCancel={() => {
              if (!linkBusy) {
                setLinkDraft(null);
                setLinkError(null);
              }
            }}
            onSubmit={(target) => void createLinkFromSelection(target)}
          />,
          document.body,
        )}
    </div>
  );
}
