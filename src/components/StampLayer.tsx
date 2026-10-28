import {
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";

import {
  createStampAnnotation,
  toCommandError,
  transformStampAnnotation,
  type AnnotationView,
  type StampFrameView,
  type StampKind,
} from "../lib/backend";
import { notifyDocumentChanged } from "../lib/documentEvents";
import { STAMP_LABELS } from "../lib/stampCatalog";
import {
  fitStampToPage,
  moveStamp,
  placeStamp,
  resizeStamp,
  rotationFromPointer,
  type Point,
} from "../lib/stampGeometry";

type StampInteraction =
  | {
      kind: "move";
      stamp: AnnotationView;
      start: Point;
      current: Point;
    }
  | {
      kind: "resize" | "rotate";
      stamp: AnnotationView;
      current: Point;
    };

export interface StampLayerProps {
  documentId: string;
  pageIndex: number;
  /** Display-space page size in points. */
  pageWidth: number;
  pageHeight: number;
  /** Permission Restrictions forbid changing document content. */
  readOnly: boolean;
  /** The PDF security handler allows adding and modifying annotations. */
  canAnnotate: boolean;
  /** Document-wide annotation list; only this page's stamps are drawn. */
  annotations: AnnotationView[];
  /** Annotation selected in the list, whose handles this layer shows. */
  selectedName: string | null;
  /** Stamp kind waiting to be placed, or null. */
  armed: StampKind | null;
  onSelect: (annotation: AnnotationView | null) => void;
  /** Called after a stamp is placed so the owner can select it. */
  onPlaced: (annotation: AnnotationView) => void;
  onArmedChange: (kind: StampKind | null) => void;
  /** Asks the owner to refetch after a mutation. */
  onChanged: () => void;
}

function stampFrame(annotation: AnnotationView): StampFrameView | null {
  return annotation.stamp?.frame ?? null;
}

/**
 * Interactive stamp layer over the rendered page.
 *
 * The stamp ink itself is already part of the page bitmap, so this layer only
 * adds transparent hit targets, the selection frame and its handles. Moving,
 * resizing and rotating are previewed locally and committed once on pointer
 * release, producing one undo step per gesture.
 */
export function StampLayer({
  documentId,
  pageIndex,
  pageWidth,
  pageHeight,
  readOnly,
  canAnnotate,
  annotations,
  selectedName,
  armed,
  onSelect,
  onPlaced,
  onArmedChange,
  onChanged,
}: StampLayerProps) {
  const layerRef = useRef<HTMLDivElement | null>(null);
  const [interaction, setInteraction] = useState<StampInteraction | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const editable = canAnnotate && !readOnly;

  const pageStamps = annotations.filter(
    (annotation) =>
      annotation.pageIndex === pageIndex &&
      annotation.kind === "stamp" &&
      annotation.stamp !== null,
  );

  function pointFromEvent(event: {
    clientX: number;
    clientY: number;
  }): Point | null {
    const layer = layerRef.current;
    if (!layer || pageWidth <= 0 || pageHeight <= 0) {
      return null;
    }
    const box = layer.getBoundingClientRect();
    if (box.width <= 0 || box.height <= 0) {
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

  function previewFrame(annotation: AnnotationView): StampFrameView | null {
    const frame = stampFrame(annotation);
    if (!frame || interaction?.stamp.name !== annotation.name) {
      return frame;
    }
    switch (interaction.kind) {
      case "move":
        return moveStamp(
          frame,
          {
            x: interaction.current.x - interaction.start.x,
            y: interaction.current.y - interaction.start.y,
          },
          pageWidth,
          pageHeight,
        );
      case "resize":
        return fitStampToPage(
          resizeStamp(frame, interaction.current),
          pageWidth,
          pageHeight,
        );
      case "rotate":
        return {
          ...frame,
          rotation: rotationFromPointer(frame, interaction.current),
        };
    }
  }

  async function commit(stamp: AnnotationView, frame: StampFrameView) {
    const original = stampFrame(stamp);
    if (
      !original ||
      (frame.x === original.x &&
        frame.y === original.y &&
        frame.width === original.width &&
        frame.height === original.height &&
        frame.rotation === original.rotation)
    ) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await transformStampAnnotation(documentId, {
        name: stamp.name,
        frame,
      });
      onChanged();
      notifyDocumentChanged({ documentId, pageIndex });
    } catch (caught) {
      setError(toCommandError(caught).message);
    } finally {
      setBusy(false);
    }
  }

  function startMove(
    event: ReactPointerEvent<HTMLDivElement>,
    stamp: AnnotationView,
  ) {
    if (armed) {
      // Let the placement handler on the layer take the click.
      return;
    }
    if (event.button !== 0) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    onSelect(stamp);
    if (!editable || busy) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    setInteraction({ kind: "move", stamp, start: point, current: point });
    try {
      event.currentTarget.setPointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture is unavailable in some embedded webviews.
    }
  }

  function startHandle(
    event: ReactPointerEvent<HTMLButtonElement>,
    stamp: AnnotationView,
    kind: "resize" | "rotate",
  ) {
    if (event.button !== 0) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    onSelect(stamp);
    if (!editable || busy) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    setInteraction({ kind, stamp, current: point });
    try {
      event.currentTarget.setPointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture is unavailable in some embedded webviews.
    }
  }

  function handlePointerMove(event: ReactPointerEvent<HTMLDivElement>) {
    if (!interaction) {
      return;
    }
    const point = pointFromEvent(event);
    if (point) {
      setInteraction({ ...interaction, current: point });
    }
  }

  function endInteraction(event: ReactPointerEvent<HTMLDivElement>) {
    if (!interaction) {
      return;
    }
    const current = pointFromEvent(event) ?? interaction.current;
    const frame = stampFrame(interaction.stamp);
    setInteraction(null);
    try {
      event.currentTarget.releasePointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture may never have been acquired.
    }
    if (!frame) {
      return;
    }
    switch (interaction.kind) {
      case "move":
        void commit(
          interaction.stamp,
          moveStamp(
            frame,
            {
              x: current.x - interaction.start.x,
              y: current.y - interaction.start.y,
            },
            pageWidth,
            pageHeight,
          ),
        );
        break;
      case "resize":
        void commit(
          interaction.stamp,
          fitStampToPage(resizeStamp(frame, current), pageWidth, pageHeight),
        );
        break;
      case "rotate":
        void commit(interaction.stamp, {
          ...frame,
          rotation: rotationFromPointer(
            frame,
            current,
            event.shiftKey ? 15 : 0,
          ),
        });
        break;
    }
  }

  function cancelInteraction(event: ReactPointerEvent<HTMLDivElement>) {
    if (!interaction) {
      return;
    }
    setInteraction(null);
    try {
      event.currentTarget.releasePointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture may never have been acquired.
    }
  }

  function place(event: ReactPointerEvent<HTMLDivElement>) {
    if (!armed || !editable || busy || event.button !== 0) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    const kind = armed;
    const frame = placeStamp(point, pageWidth, pageHeight);
    setBusy(true);
    setError(null);
    createStampAnnotation(documentId, {
      pageIndex,
      kind,
      opacity: 100,
      frame,
    })
      .then((mutation) => {
        onChanged();
        notifyDocumentChanged({ documentId, pageIndex });
        onArmedChange(null);
        if (mutation.annotation) {
          onPlaced(mutation.annotation);
        }
      })
      .catch((caught) => {
        setError(toCommandError(caught).message);
      })
      .finally(() => {
        setBusy(false);
      });
  }

  return (
    <div
      ref={layerRef}
      className={`stamp-layer${armed && editable ? " stamp-layer--armed" : ""}`}
      data-testid="stamp-layer"
      aria-label="图章层"
      onPointerDown={place}
      onPointerMove={handlePointerMove}
      onPointerUp={endInteraction}
      onPointerCancel={cancelInteraction}
    >
      {pageStamps.map((annotation) => {
        const frame = previewFrame(annotation);
        if (!frame) {
          return null;
        }
        const selected = annotation.name === selectedName;
        const kind = annotation.stamp?.kind;
        return (
          <div
            key={annotation.name}
            className={`stamp-hit${selected ? " stamp-hit--selected" : ""}`}
            data-testid={`stamp-hit-${annotation.name}`}
            aria-label={
              kind ? `图章：${STAMP_LABELS[kind]}` : "图章"
            }
            style={{
              left: `${(frame.x / pageWidth) * 100}%`,
              top: `${(frame.y / pageHeight) * 100}%`,
              width: `${(frame.width / pageWidth) * 100}%`,
              height: `${(frame.height / pageHeight) * 100}%`,
              transform: `rotate(${frame.rotation}deg)`,
              borderColor: annotation.color,
            }}
            onPointerDown={(event) => startMove(event, annotation)}
          >
            {selected && editable && interaction === null && (
              <>
                <button
                  type="button"
                  className="stamp-handle stamp-handle--rotate"
                  aria-label="旋转图章"
                  data-testid="stamp-rotate-handle"
                  onPointerDown={(event) =>
                    startHandle(event, annotation, "rotate")
                  }
                  onDoubleClick={(event) => event.stopPropagation()}
                />
                <button
                  type="button"
                  className="stamp-handle stamp-handle--resize"
                  aria-label="调整图章大小"
                  data-testid="stamp-resize-handle"
                  onPointerDown={(event) =>
                    startHandle(event, annotation, "resize")
                  }
                  onDoubleClick={(event) => event.stopPropagation()}
                />
              </>
            )}
          </div>
        );
      })}

      {error && (
        <p className="stamp-layer-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
