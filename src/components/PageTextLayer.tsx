import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";

import {
  applyTextBoxStyle,
  applyTextEdit,
  createInkAnnotation,
  createNoteAnnotation,
  createTextBox,
  createTextMarkupAnnotation,
  deleteTextBox,
  editHistory,
  extractTextLayer,
  redoTextEdit,
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
  type InkPoint,
  type TextBoxFontView,
  type TextBoxStyleView,
  type TextBoxTargetView,
  type TextLayerView,
  type TextMarkupKind,
  type TextMarkupRect,
} from "../lib/backend";
import { notifyDocumentChanged, onDocumentChanged } from "../lib/documentEvents";
import {
  dragRect,
  estimatedContentHeight,
  fitBoxToPage,
  moveBox,
  resizeBox,
  rotationFromPointer,
  type BoxRect,
  type Point as BoxPoint,
} from "../lib/textBoxGeometry";
import { TextBoxStylePanel } from "./TextBoxStylePanel";
import {
  buildTextLayer,
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
  history,
  notice,
  onToggleEdit,
  onToggleBoxMode,
  onToggleNoteMode,
  onToggleInkMode,
  onInkColor,
  onInkOpacity,
  onInkLineWidth,
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
}) {
  const annotationDisabled = !canAnnotate || readOnly || busy;
  return (
    <div
      className="text-edit-toolbar"
      onPointerDown={(event) => event.stopPropagation()}
    >
      <button
        type="button"
        className="text-edit-toggle"
        aria-pressed={editMode}
        disabled={!canEdit || readOnly || busy}
        title={
          readOnly
            ? READ_ONLY_REASON
            : !canEdit
              ? SCANNED_REASON
              : editMode
                ? "退出文字编辑"
                : "点击页面上的文字开始编辑"
        }
        onClick={onToggleEdit}
      >
        编辑文字
      </button>
      <button
        type="button"
        className="text-box-toggle"
        aria-pressed={boxMode}
        disabled={!canEdit || readOnly || busy}
        title={
          readOnly
            ? READ_ONLY_REASON
            : !canEdit
              ? SCANNED_REASON
              : boxMode
                ? "退出文本框编辑"
                : "在页面上点击或拖动以添加文本框"
        }
        onClick={onToggleBoxMode}
      >
        添加文本框
      </button>
      <span className="toolbar-divider" aria-hidden="true" />
      <button
        type="button"
        className="annotation-note-toggle"
        aria-pressed={noteMode}
        disabled={annotationDisabled}
        title={
          readOnly
            ? READ_ONLY_REASON
            : !canAnnotate
              ? ANNOTATION_PERMISSION_REASON
              : noteMode
                ? "退出便签放置"
                : "在页面上点击以放置便签"
        }
        onClick={onToggleNoteMode}
      >
        便签
      </button>
      <button
        type="button"
        className="annotation-ink-toggle"
        aria-pressed={inkMode}
        disabled={annotationDisabled}
        title={
          readOnly
            ? READ_ONLY_REASON
            : !canAnnotate
              ? ANNOTATION_PERMISSION_REASON
              : inkMode
                ? "退出手绘"
                : "在页面上按住并拖动以手绘"
        }
        onClick={onToggleInkMode}
      >
        手绘
      </button>
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
      <span className="toolbar-divider" aria-hidden="true" />
      <button
        type="button"
        disabled={!history?.canUndo || busy}
        title={history?.canUndo ? "撤销上一步编辑" : "没有可撤销的操作"}
        onClick={onUndo}
      >
        撤销
      </button>
      <button
        type="button"
        disabled={!history?.canRedo || busy}
        title={history?.canRedo ? "重做上一步编辑" : "没有可重做的操作"}
        onClick={onRedo}
      >
        重做
      </button>
      {readOnly && <span className="text-edit-reason">{READ_ONLY_REASON}</span>}
      {notice && (
        <span
          className={`text-edit-notice text-edit-notice--${notice.kind}`}
          role={notice.kind === "error" ? "alert" : "status"}
        >
          {notice.text}
        </span>
      )}
    </div>
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
}: PageTextLayerProps) {
  const [state, setState] = useState<LayerState>({ status: "loading" });
  const [selection, setSelection] = useState<TextRange | null>(null);
  const [copyStatus, setCopyStatus] = useState<"idle" | "copied" | "failed">("idle");
  const [markupColor, setMarkupColor] = useState(DEFAULT_MARKUP_COLOR);
  const [markupOpacity, setMarkupOpacity] = useState(DEFAULT_MARKUP_OPACITY);
  const [markupBusy, setMarkupBusy] = useState(false);
  const [noteMode, setNoteMode] = useState(false);
  const [inkMode, setInkMode] = useState(false);
  const [inkColor, setInkColor] = useState(DEFAULT_INK_COLOR);
  const [inkOpacity, setInkOpacity] = useState(DEFAULT_INK_OPACITY);
  const [inkLineWidth, setInkLineWidth] = useState(DEFAULT_INK_LINE_WIDTH);
  const [inkDraft, setInkDraft] = useState<InkPoint[] | null>(null);
  const [inkBusy, setInkBusy] = useState(false);
  const [noteInteraction, setNoteInteraction] = useState<NoteInteraction | null>(
    null,
  );
  const [noteBusy, setNoteBusy] = useState(false);
  const [editMode, setEditMode] = useState(false);
  const [boxMode, setBoxMode] = useState(false);
  const [boxes, setBoxes] = useState<TextBoxTargetView[]>([]);
  const [fonts, setFonts] = useState<TextBoxFontView[]>([]);
  const [selectedBoxId, setSelectedBoxId] = useState<number | null>(null);
  const [boxInteraction, setBoxInteraction] = useState<BoxInteraction | null>(
    null,
  );
  const [boxBusy, setBoxBusy] = useState(false);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [history, setHistory] = useState<EditHistoryView | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  // External overlays (shapes, the annotation list) change the same document
  // and share this undo stack, so their events re-fetch the history. This
  // layer's own commands already return the new history, and re-fetching it
  // here could overwrite that with a slower, stale response.
  const [historyRefreshKey, setHistoryRefreshKey] = useState(0);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const dragAnchor = useRef<number | null>(null);
  const targetRequest = useRef(0);
  const loadedPageKey = useRef<string | null>(null);

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
    if (!boxMode) {
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
  }, [boxMode, documentId, pageIndex, reloadKey]);

  useEffect(() => {
    if (!boxMode) {
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
  }, [boxMode]);

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
  }, [editor]);

  const layer = state.status === "ready" ? state.layer : null;
  const structure = useMemo(() => (layer ? buildTextLayer(layer.chars) : null), [layer]);
  const selectedBox =
    boxMode && selectedBoxId !== null
      ? (boxes.find((box) => box.boxId === selectedBoxId) ?? null)
      : null;

  if (state.status === "loading") {
    return null;
  }

  if (state.status === "error") {
    return (
      <div className="page-text-layer page-text-layer--message" role="status" title={state.error.message}>
        <span className="text-layer-message">文字层无法载入</span>
      </div>
    );
  }

  const editingBusy = editor?.status === "editing" && editor.busy;
  const operationBusy =
    Boolean(editingBusy) || boxBusy || markupBusy || inkBusy || noteBusy;
  const annotationTools = canAnnotate && !readOnly;
  const toolbarFor = (canEdit: boolean) => (
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
      const result = await createTextBox(documentId, pageIndex, rect);
      setHistory(result.history);
      setBoxes((current) => [
        ...current.filter((box) => box.boxId !== result.target.boxId),
        result.target,
      ]);
      setSelectedBoxId(result.target.boxId);
      setEditor({
        status: "editing",
        target: result.target,
        draft: "",
        busy: false,
        error: null,
      });
      setNotice(
        result.font?.fallback
          ? {
              kind: "info",
              text: `${result.font.originalFamily} 无法用于新文字，已改用 ${result.font.family}${result.font.reason ? `：${result.font.reason}` : ""}`,
            }
          : null,
      );
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
  ) {
    setBoxBusy(true);
    setNotice(null);
    try {
      const result = await transformTextBox(
        documentId,
        pageIndex,
        box.boxId,
        {
          x: rect.x,
          y: rect.y,
          width: rect.width,
          height: rect.height,
          autoHeight,
          rotation,
        },
      );
      setHistory(result.history);
      setBoxes((current) =>
        current.map((item) =>
          item.boxId === result.target.boxId ? result.target : item,
        ),
      );
      notifyDocumentChanged({ documentId, pageIndex, source: "text" });
    } catch (error) {
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
      notifyDocumentChanged({ documentId, pageIndex, source: "text" });
    } catch (error) {
      setNotice({ kind: "error", text: toCommandError(error).message });
    } finally {
      setBoxBusy(false);
    }
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
      setBoxes((current) =>
        current.filter((item) => item.boxId !== box.boxId),
      );
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

  async function commitEditor() {
    if (!editor || editor.status !== "editing" || editor.busy) {
      return;
    }
    const current = editor;
    if (!current.draft.trim() && !current.target.created) {
      setEditor({
        ...current,
        error: "文字不能为空；如需移除文字，请使用删除。",
      });
      return;
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
      setNotice(
        result.font?.fallback
          ? {
              kind: "info",
              text: `${result.font.originalFamily} 无法用于新文字，已改用 ${result.font.family}${result.font.reason ? `：${result.font.reason}` : ""}`,
            }
          : null,
      );
      notifyDocumentChanged({ documentId, pageIndex, source: "text" });
    } catch (error) {
      setEditor({
        ...current,
        busy: false,
        error: toCommandError(error).message,
      });
    }
  }

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
    const width = Math.max(
      1,
      Math.min(DEFAULT_NOTE_SIZE, textLayer.pageWidth),
    );
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
    setBoxInteraction({ kind: "resize", box, start: point, current: point });
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
      if (editor || boxBusy) {
        return;
      }
      event.preventDefault();
      surfaceRef.current?.focus({ preventScroll: true });
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
      surfaceRef.current?.focus({ preventScroll: true });
      if (editor) {
        return;
      }
      const index = findCharAt(textLayer, point);
      if (index !== null) {
        void openEditor(index);
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
        setBoxInteraction({ ...boxInteraction, current: point });
      }
      return;
    }
    if (boxInteraction?.kind === "move") {
      const point = pointFromEvent(event);
      if (point) {
        setBoxInteraction({ ...boxInteraction, current: point });
      }
      return;
    }
    if (boxInteraction?.kind === "resize") {
      const point = pointFromEvent(event);
      if (point) {
        setBoxInteraction({ ...boxInteraction, current: point });
      }
      return;
    }
    if (boxInteraction?.kind === "rotate") {
      const point = pointFromEvent(event);
      if (point) {
        setBoxInteraction({ ...boxInteraction, current: point });
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
      const resized = resizeBox(
        interaction.box,
        interaction.box.rotation,
        point,
      );
      if (
        resized.width !== interaction.box.width ||
        resized.height !== interaction.box.height
      ) {
        void transformBox(interaction.box, resized, false);
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
        void transformBox(interaction.box, moved);
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
      const rect = fitBoxToPage(
        dragRect(current.anchor, point),
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
    if (editMode || boxMode || noteMode || inkMode) {
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

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
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
    const modifier = event.metaKey || event.ctrlKey;
    if (modifier && event.key.toLowerCase() === "z") {
      event.preventDefault();
      void runHistory(event.shiftKey ? "redo" : "undo");
      return;
    }
    if (modifier && event.key.toLowerCase() === "y") {
      event.preventDefault();
      void runHistory("redo");
      return;
    }
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
  const previewRotationFor = (box: TextBoxTargetView): number => {    if (
      boxInteraction?.kind === "rotate" &&
      boxInteraction.box.boxId === box.boxId
    ) {
      return rotationFromPointer(box, boxInteraction.current);
    }
    return box.rotation;
  };
  const previewRectFor = (box: TextBoxTargetView): BoxRect => {
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
      return resizeBox(box, box.rotation, boxInteraction.current);
    }
    return box;
  };

  return (
    <div
      ref={surfaceRef}
      className={`page-text-layer${editMode ? " page-text-layer--editing" : ""}${
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
      {toolbarFor(textLayer.hasText)}

      {!textLayer.hasText && (
        <span className="text-layer-message" role="status">
          {SCANNED_REASON}
        </span>
      )}

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
              points={inkDraft.map((point) => `${point.x},${point.y}`).join(" ")}
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

      {boxMode && (
        <div className="text-box-overlay">
          {boxes.map((box, index) => {
            const rect = previewRectFor(box);
            return (
              <div
                key={box.boxId}
                className={`text-box-outline${
                  selectedBoxId === box.boxId
                    ? " text-box-outline--selected"
                    : ""
                }`}
                role="group"
                aria-label={`文本框 ${index + 1}`}
                data-testid={`text-box-outline-${box.boxId}`}
                style={{
                  left: `${(rect.x / textLayer.pageWidth) * 100}%`,
                  top: `${(rect.y / textLayer.pageHeight) * 100}%`,
                  width: `${(rect.width / textLayer.pageWidth) * 100}%`,
                  height: `${(rect.height / textLayer.pageHeight) * 100}%`,
                  transform: `rotate(${previewRotationFor(box)}deg)`,
                }}
                onPointerDown={(event) => startBoxMove(event, box)}
                onDoubleClick={(event) => {
                  event.stopPropagation();
                  openBoxEditor(box);
                }}
              >
                <span className="text-box-outline-label">
                  {box.text ? box.text.split("\n")[0] : "空文本框"}
                </span>
                {selectedBoxId === box.boxId && (
                  <>
                    <button
                      type="button"
                      className="text-box-handle text-box-handle--rotate"
                      aria-label="旋转"
                      data-testid="text-box-rotate-handle"
                      onPointerDown={(event) => startBoxRotate(event, box)}
                      onDoubleClick={(event) => event.stopPropagation()}
                    />
                    <button
                      type="button"
                      className="text-box-handle text-box-handle--resize"
                      aria-label="调整大小"
                      data-testid="text-box-resize-handle"
                      onPointerDown={(event) => startBoxResize(event, box)}
                      onDoubleClick={(event) => event.stopPropagation()}
                    />
                  </>
                )}
              </div>
            );
          })}
        </div>
      )}

      {boxMode && selectedBox && editor?.status !== "editing" && (
        <TextBoxStylePanel
          target={selectedBox}
          fonts={fonts}
          busy={boxBusy}
          onChange={(style) => void applyBoxStyle(style)}
          onDelete={() => void deleteSelectedBox()}
        />
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
            autoFocus
            spellCheck={false}
            style={{
              fontFamily: editor.target.fontFamily,
              fontSize: `${editor.target.fontSize * scale}px`,
              lineHeight: 1.2,
              color: editor.target.color,
            }}
            onChange={(event) =>
              setEditor({ ...editor, draft: event.target.value, error: null })
            }
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.preventDefault();
                event.stopPropagation();
                cancelEditor();
              } else if (
                (event.metaKey || event.ctrlKey) &&
                event.key === "Enter"
              ) {
                event.preventDefault();
                void commitEditor();
              }
            }}
          />
          <div className="text-box-actions">
            <button
              type="button"
              disabled={editor.busy}
              onClick={() => void commitEditor()}
            >
              应用
            </button>
            <button
              type="button"
              disabled={editor.busy}
              onClick={() => void removeEditor()}
            >
              删除
            </button>
            <button
              type="button"
              disabled={editor.busy}
              onClick={cancelEditor}
            >
              取消
            </button>
          </div>
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
          <span className="text-selection-count">已选择 {selectedCharacterCount} 个字符</span>
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
            </span>
          ) : (
            <span className="text-annotation-reason">
              {ANNOTATION_PERMISSION_REASON}
            </span>
          )}
        </div>
      )}
    </div>
  );
}
