import {
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";

import {
  deleteAnnotation,
  eraseInkStroke,
  toCommandError,
  type AnnotationView,
} from "../lib/backend";
import { notifyDocumentChanged } from "../lib/documentEvents";
import { hitTestEraser } from "../lib/eraserHitTest";

const DEFAULT_RADIUS = 8;
const MIN_RADIUS = 2;
const MAX_RADIUS = 60;

type Notice = { kind: "info" | "error"; text: string };

export interface EraserLayerProps {
  documentId: string;
  pageIndex: number;
  /** Display-space page size, matching the text layer coordinate system. */
  pageWidth: number;
  pageHeight: number;
  /** Permission Restrictions deny every editing capability. */
  readOnly: boolean;
  /** The PDF security handler allows adding and modifying annotations. */
  canAnnotate: boolean;
  /** Document-wide annotation list; only this page's annotations are hit. */
  annotations: AnnotationView[];
  selectedName: string | null;
  onSelectAnnotation: (annotation: AnnotationView | null) => void;
  /** Asks the owner to refetch after a mutation. */
  onChanged: () => void;
}

/**
 * Eraser overlay for every annotation kind.
 *
 * Clicking a non-ink annotation deletes it whole. Clicking ink removes only
 * the stroke under the pointer; the backend rebuilds `/InkList`, `/Rect` and
 * the appearance stream and records the stroke removal as one undo step.
 * Shift-click erases a whole ink annotation. The eraser only arms on request
 * so it never blocks text selection or the other overlays.
 */
export function EraserLayer({
  documentId,
  pageIndex,
  pageWidth,
  pageHeight,
  readOnly,
  canAnnotate,
  annotations,
  selectedName,
  onSelectAnnotation,
  onChanged,
}: EraserLayerProps) {
  const surfaceRef = useRef<HTMLDivElement | null>(null);
  const [active, setActive] = useState(false);
  const [radius, setRadius] = useState(DEFAULT_RADIUS);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const editable = canAnnotate && !readOnly;

  if (!editable) {
    return null;
  }

  function pointFromEvent(event: {
    clientX: number;
    clientY: number;
  }): { x: number; y: number } | null {
    const box = surfaceRef.current?.getBoundingClientRect();
    if (!box || box.width <= 0 || box.height <= 0) {
      return null;
    }
    if (pageWidth <= 0 || pageHeight <= 0) {
      return null;
    }
    return {
      x: Math.min(
        Math.max(((event.clientX - box.left) / box.width) * pageWidth, 0),
        pageWidth,
      ),
      y: Math.min(
        Math.max(((event.clientY - box.top) / box.height) * pageHeight, 0),
        pageHeight,
      ),
    };
  }

  async function erase(event: ReactPointerEvent<HTMLDivElement>) {
    if (!active || busy || event.button !== 0) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    const hit = hitTestEraser(annotations, point, {
      radius,
      wholeInk: event.shiftKey,
      pageIndex,
    });
    if (!hit) {
      setNotice({ kind: "info", text: "此处没有可擦除的标注" });
      return;
    }

    setBusy(true);
    setNotice(null);
    try {
      if (hit.strokeIndex !== null) {
        const mutated = await eraseInkStroke(
          documentId,
          hit.annotation.name,
          hit.strokeIndex,
        );
        onChanged();
        notifyDocumentChanged({ documentId, pageIndex, source: "annotation" });
        if (selectedName === hit.annotation.name) {
          onSelectAnnotation(mutated.annotation);
        }
      } else {
        await deleteAnnotation(documentId, hit.annotation.name);
        onChanged();
        notifyDocumentChanged({ documentId, pageIndex, source: "annotation" });
        if (selectedName === hit.annotation.name) {
          onSelectAnnotation(null);
        }
      }
    } catch (caught) {
      setNotice({ kind: "error", text: toCommandError(caught).message });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      className={`eraser-layer${active ? " eraser-layer--active" : ""}`}
      data-testid="eraser-layer"
    >
      <div
        className="eraser-toolbar"
        role="toolbar"
        aria-label="橡皮擦工具"
        onPointerDown={(event) => event.stopPropagation()}
      >
        <span className="eraser-toolbar-title">橡皮擦</span>
        <button
          type="button"
          aria-pressed={active}
          disabled={busy}
          title="单击删除标注；单击墨迹只擦除该笔画，Shift+单击删除整条墨迹"
          onClick={() => {
            setNotice(null);
            setActive((current) => !current);
          }}
        >
          橡皮擦
        </button>
        <label className="eraser-toolbar-control">
          半径
          <input
            type="number"
            aria-label="橡皮擦半径"
            min={MIN_RADIUS}
            max={MAX_RADIUS}
            step={1}
            value={radius}
            disabled={busy}
            onChange={(event) => {
              const next = Number.parseFloat(event.target.value);
              if (Number.isFinite(next)) {
                setRadius(Math.min(Math.max(next, MIN_RADIUS), MAX_RADIUS));
              }
            }}
          />
        </label>
        {active && (
          <span className="eraser-toolbar-hint">
            单击墨迹擦除笔画，Shift+单击删除整条标注
          </span>
        )}
      </div>

      {active && (
        <div
          ref={surfaceRef}
          className="eraser-surface"
          data-testid="eraser-surface"
          aria-label="擦除页面标注"
          onPointerDown={erase}
          onContextMenu={(event) => event.preventDefault()}
        />
      )}

      {notice && (
        <p
          className={`eraser-notice eraser-notice--${notice.kind}${
            active ? "" : " eraser-notice--idle"
          }`}
          role={notice.kind === "error" ? "alert" : "status"}
        >
          {notice.text}
        </p>
      )}
    </div>
  );
}
