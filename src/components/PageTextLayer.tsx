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
  applyTextEdit,
  deleteTextBox,
  editHistory,
  extractTextLayer,
  redoTextEdit,
  textEditTarget,
  toCommandError,
  undoTextEdit,
  type CommandError,
  type EditHistoryView,
  type TextBoxTargetView,
  type TextLayerView,
} from "../lib/backend";
import { notifyDocumentChanged, onDocumentChanged } from "../lib/documentEvents";
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

const READ_ONLY_REASON = "此文档不允许编辑内容";
const SCANNED_REASON = "此页没有可编辑文字";

export interface PageTextLayerProps {
  documentId: string;
  pageIndex: number;
  /** Permission Restrictions forbid changing document content. */
  readOnly?: boolean;
  /** Page render scale, used to size the inline editor text. */
  scale?: number;
}

function TextEditToolbar({
  canEdit,
  editMode,
  readOnly,
  busy,
  history,
  notice,
  onToggleEdit,
  onUndo,
  onRedo,
}: {
  canEdit: boolean;
  editMode: boolean;
  readOnly: boolean;
  busy: boolean;
  history: EditHistoryView | null;
  notice: Notice | null;
  onToggleEdit: () => void;
  onUndo: () => void;
  onRedo: () => void;
}) {
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
      <span className="toolbar-divider" aria-hidden="true" />
      <button
        type="button"
        disabled={!history?.canUndo || busy}
        title={history?.canUndo ? "撤销上一步文字编辑" : "没有可撤销的操作"}
        onClick={onUndo}
      >
        撤销
      </button>
      <button
        type="button"
        disabled={!history?.canRedo || busy}
        title={history?.canRedo ? "重做上一步文字编辑" : "没有可重做的操作"}
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
 */
export function PageTextLayer({
  documentId,
  pageIndex,
  readOnly = false,
  scale = 1,
}: PageTextLayerProps) {
  const [state, setState] = useState<LayerState>({ status: "loading" });
  const [selection, setSelection] = useState<TextRange | null>(null);
  const [copyStatus, setCopyStatus] = useState<"idle" | "copied" | "failed">("idle");
  const [editMode, setEditMode] = useState(false);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [history, setHistory] = useState<EditHistoryView | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
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
  }, [documentId]);

  useEffect(
    () =>
      onDocumentChanged((detail) => {
        if (detail.documentId === documentId && detail.pageIndex === pageIndex) {
          setReloadKey((key) => key + 1);
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
  const toolbarFor = (canEdit: boolean) => (
    <TextEditToolbar
      canEdit={canEdit}
      editMode={editMode}
      readOnly={readOnly}
      busy={editingBusy}
      history={history}
      notice={notice}
      onToggleEdit={toggleEditMode}
      onUndo={() => void runHistory("undo")}
      onRedo={() => void runHistory("redo")}
    />
  );

  if (!layer || !structure || !layer.hasText) {
    return (
      <div className="page-text-layer page-text-layer--message">
        <span className="text-layer-message" role="status">
          {SCANNED_REASON}
        </span>
        <div className="text-edit-toolbar-holder">{toolbarFor(false)}</div>
      </div>
    );
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
    setSelection(null);
    setCopyStatus("idle");
    setNotice(null);
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

  async function commitEditor() {
    if (!editor || editor.status !== "editing" || editor.busy) {
      return;
    }
    const current = editor;
    if (!current.draft.trim()) {
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
      setNotice(
        result.font?.fallback
          ? {
              kind: "info",
              text: `${result.font.originalFamily} 无法用于新文字，已改用 ${result.font.family}${result.font.reason ? `：${result.font.reason}` : ""}`,
            }
          : null,
      );
      notifyDocumentChanged({ documentId, pageIndex });
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
      setNotice(null);
      notifyDocumentChanged({ documentId, pageIndex });
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
      setNotice(null);
      notifyDocumentChanged({
        documentId,
        pageIndex: step.pageIndex,
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

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
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
    if (editMode || dragAnchor.current === null) {
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
    dragAnchor.current = null;
    try {
      event.currentTarget.releasePointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture may never have been acquired.
    }
  };

  const handleDoubleClick = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (editMode) {
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
    if (editMode || event.detail !== 3) {
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

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      if (editor) {
        event.preventDefault();
        cancelEditor();
        return;
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
  const selectedCharacterCount = selection
    ? [...selectionText(textLayer, selection)].length
    : 0;

  return (
    <div
      ref={surfaceRef}
      className={`page-text-layer${editMode ? " page-text-layer--editing" : ""}`}
      role="region"
      aria-label="文本层"
      tabIndex={0}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onDoubleClick={handleDoubleClick}
      onClick={handleClick}
      onKeyDown={handleKeyDown}
    >
      {toolbarFor(true)}

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

      {editor?.status === "editing" && (
        <div
          className="text-box-editor"
          data-testid="text-box-editor"
          onPointerDown={(event) => event.stopPropagation()}
          style={{
            left: `${(editor.target.x / textLayer.pageWidth) * 100}%`,
            top: `${(editor.target.y / textLayer.pageHeight) * 100}%`,
            width: `${(editor.target.width / textLayer.pageWidth) * 100}%`,
            minHeight: `${Math.max(editor.target.height, editor.target.fontSize * 1.2) * scale}px`,
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
        </div>
      )}
    </div>
  );
}
