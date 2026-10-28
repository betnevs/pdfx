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
  extractTextLayer,
  toCommandError,
  type CommandError,
  type TextLayerView,
} from "../lib/backend";
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

export interface PageTextLayerProps {
  documentId: string;
  pageIndex: number;
}

/**
 * Transparent interaction layer over the rendered page image.
 *
 * Geometry from the backend is in display-space page points, so highlight
 * rectangles are positioned as percentages of the page box. That keeps the
 * layer aligned at every zoom level without re-fetching text when zoom
 * changes, and the page rotation/CropBox are already applied by the backend.
 */
export function PageTextLayer({ documentId, pageIndex }: PageTextLayerProps) {
  const [state, setState] = useState<LayerState>({ status: "loading" });
  const [selection, setSelection] = useState<TextRange | null>(null);
  const [copyStatus, setCopyStatus] = useState<"idle" | "copied" | "failed">("idle");
  const surfaceRef = useRef<HTMLDivElement>(null);
  const dragAnchor = useRef<number | null>(null);

  useEffect(() => {
    let cancelled = false;
    setState({ status: "loading" });
    setSelection(null);
    setCopyStatus("idle");

    extractTextLayer(documentId, pageIndex)
      .then((layer) => {
        if (!cancelled) {
          setState({ status: "ready", layer });
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
  }, [documentId, pageIndex]);

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

  if (!layer || !structure || !layer.hasText) {
    return (
      <div className="page-text-layer page-text-layer--message" role="status">
        <span className="text-layer-message">此页没有可编辑文字</span>
      </div>
    );
  }

  const textLayer: TextLayerView = layer;
  const textStructure: TextLayerStructure = structure;

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
    if (dragAnchor.current === null) {
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
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    setCopyStatus("idle");
    setSelection(selectAt(point, "word"));
  };

  const handleClick = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (event.detail !== 3) {
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
      setSelection(null);
      setCopyStatus("idle");
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
      className="page-text-layer"
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
