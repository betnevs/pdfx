import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
} from "react";

import {
  createShapeAnnotation,
  toCommandError,
  updateShapeGeometry,
  type AnnotationView,
  type ShapeGeometry,
  type ShapeKind,
  type ShapeStyle,
} from "../lib/backend";
import { notifyDocumentChanged } from "../lib/documentEvents";
import {
  moveShape,
  resizeShape,
  shapeRectFromDrag,
  shapeRotationFromPointer,
  type Point,
  type ShapeGeometryLike,
  type ShapeRect,
} from "../lib/shapeGeometry";
import { ToolShelf, type ToolShelfTool } from "./ToolShelf";

/** Defaults chosen to be visible on both light and dark pages. */
const DEFAULT_FILL = "#3366cc";
const DEFAULT_STROKE = "#000000";
const DEFAULT_OPACITY = 100;
const DEFAULT_LINE_WIDTH = 2;
/** Screen-space distance from the top edge to the rotate handle, in points. */
const ROTATE_HANDLE_OFFSET = 18;
const MIN_LINE_WIDTH = 0.5;
const MAX_LINE_WIDTH = 100;
/** Pointer travel below this is a click, not a transform. */
const MOVE_SLOP = 0.5;
const KIND_LABELS: Record<ShapeKind, string> = {
  square: "矩形",
  circle: "椭圆",
};

type Tool = ShapeKind | null;
type Notice = { kind: "info" | "error"; text: string };

type Interaction =
  | {
      kind: "create";
      shapeKind: ShapeKind;
      anchor: Point;
      current: Point;
    }
  | {
      kind: "move";
      annotation: AnnotationView;
      start: Point;
      current: Point;
    }
  | { kind: "resize"; annotation: AnnotationView; current: Point }
  | {
      kind: "rotate";
      annotation: AnnotationView;
      start: Point;
      startRotation: number;
      current: Point;
      snap: boolean;
    };

export interface ShapeLayerProps {
  documentId: string;
  pageIndex: number;
  /** Display-space page size, matching the text layer coordinate system. */
  pageWidth: number;
  pageHeight: number;
  /** Page render scale, used to keep handles a constant screen size. */
  scale?: number;
  /** Permission Restrictions deny every editing capability. */
  readOnly?: boolean;
  /** The PDF security handler allows adding and modifying annotations. */
  canAnnotate?: boolean;
  annotations?: AnnotationView[];
  selectedAnnotation?: AnnotationView | null;
  onSelectAnnotation?: (annotation: AnnotationView | null) => void;
}

function geometryEquals(
  left: ShapeGeometryLike,
  right: ShapeGeometryLike,
): boolean {
  return (
    Math.abs(left.x - right.x) < MOVE_SLOP &&
    Math.abs(left.y - right.y) < MOVE_SLOP &&
    Math.abs(left.width - right.width) < MOVE_SLOP &&
    Math.abs(left.height - right.height) < MOVE_SLOP &&
    Math.abs(left.rotation - right.rotation) < MOVE_SLOP
  );
}

function rotatePoint(
  x: number,
  y: number,
  centreX: number,
  centreY: number,
  degrees: number,
): Point {
  const radians = (degrees * Math.PI) / 180;
  const dx = x - centreX;
  const dy = y - centreY;
  return {
    x: centreX + dx * Math.cos(radians) - dy * Math.sin(radians),
    y: centreY + dx * Math.sin(radians) + dy * Math.cos(radians),
  };
}

function outlineGeometry(geometry: ShapeGeometryLike): ShapeRect {
  return {
    x: geometry.x,
    y: geometry.y,
    width: geometry.width,
    height: geometry.height,
  };
}

function shapeTransform(rect: ShapeRect, rotation: number): string {
  return `rotate(${rotation} ${rect.x + rect.width / 2} ${
    rect.y + rect.height / 2
  })`;
}

/**
 * Canvas overlay for native rectangle and ellipse annotations.
 *
 * The toolbar draws a live SVG preview in the same display-space geometry and
 * style the backend serializes into the appearance stream, so what the user
 * sees while dragging matches the saved page. Selecting a shape outlines its
 * rotated geometry; dragging its body moves it and the two handles resize and
 * rotate it. Every completed gesture is one unified-history action.
 */
export function ShapeLayer({
  documentId,
  pageIndex,
  pageWidth,
  pageHeight,
  scale = 1,
  readOnly = false,
  canAnnotate = false,
  annotations = [],
  selectedAnnotation = null,
  onSelectAnnotation,
}: ShapeLayerProps) {
  const [tool, setTool] = useState<Tool>(null);
  const [fillEnabled, setFillEnabled] = useState(true);
  const [strokeEnabled, setStrokeEnabled] = useState(true);
  const [fillColor, setFillColor] = useState(DEFAULT_FILL);
  const [strokeColor, setStrokeColor] = useState(DEFAULT_STROKE);
  const [opacity, setOpacity] = useState(DEFAULT_OPACITY);
  const [lineWidth, setLineWidth] = useState(DEFAULT_LINE_WIDTH);
  const [interaction, setInteraction] = useState<Interaction | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const layerRef = useRef<HTMLDivElement>(null);

  const canEdit = canAnnotate && !readOnly;
  const drawable = fillEnabled || strokeEnabled;
  const style: ShapeStyle = {
    fillColor: fillEnabled ? fillColor : null,
    strokeColor: strokeEnabled ? strokeColor : null,
    opacity,
    lineWidth,
  };

  useEffect(() => {
    if (!canEdit) {
      setTool(null);
      setInteraction(null);
    }
  }, [canEdit]);

  useEffect(() => {
    setTool(null);
    setInteraction(null);
    setNotice(null);
  }, [documentId, pageIndex]);

  useEffect(() => {
    if (!tool && !interaction) {
      return;
    }
    const cancel = (event: KeyboardEvent) => {
      if (event.key !== "Escape") {
        return;
      }
      event.preventDefault();
      setInteraction(null);
      setTool(null);
    };
    window.addEventListener("keydown", cancel);
    return () => window.removeEventListener("keydown", cancel);
  }, [tool, interaction]);

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
    if (!canEdit || !drawable || busy || tool === null || event.button !== 0) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    capturePointer(event);
    const point = pointFromEvent(event);
    setNotice(null);
    setInteraction({
      kind: "create",
      shapeKind: tool,
      anchor: point,
      current: point,
    });
  }

  function startMove(
    event: ReactPointerEvent<SVGElement>,
    annotation: AnnotationView,
  ) {
    event.stopPropagation();
    onSelectAnnotation?.(annotation);
    if (!canEdit || !annotation.shape || busy || tool || event.button !== 0) {
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
    if (!canEdit || !annotation.shape || busy) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    capturePointer(event);
    const point = pointFromEvent(event);
    setNotice(null);
    setInteraction({ kind: "resize", annotation, current: point });
  }

  function startRotate(
    event: ReactPointerEvent<HTMLButtonElement>,
    annotation: AnnotationView,
  ) {
    if (!canEdit || !annotation.shape || busy) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    capturePointer(event);
    const point = pointFromEvent(event);
    setNotice(null);
    setInteraction({
      kind: "rotate",
      annotation,
      start: point,
      startRotation: annotation.shape.geometry.rotation,
      current: point,
      snap: event.shiftKey,
    });
  }

  function moveInteraction(event: ReactPointerEvent<Element>) {
    if (!interaction) {
      return;
    }
    const point = pointFromEvent(event);
    if (interaction.kind === "create") {
      setInteraction({ ...interaction, current: point });
    } else if (interaction.kind === "rotate") {
      setInteraction({
        ...interaction,
        current: point,
        snap: event.shiftKey,
      });
    } else {
      setInteraction({ ...interaction, current: point });
    }
  }

  function previewGeometryFor(interaction: Interaction): ShapeGeometry | null {
    if (interaction.kind === "create") {
      return null;
    }
    const shape = interaction.annotation.shape;
    if (!shape) {
      return null;
    }
    if (interaction.kind === "move") {
      return moveShape(
        shape.geometry,
        {
          x: interaction.current.x - interaction.start.x,
          y: interaction.current.y - interaction.start.y,
        },
        pageWidth,
        pageHeight,
      );
    }
    if (interaction.kind === "resize") {
      return resizeShape(
        shape.geometry,
        interaction.current,
        pageWidth,
        pageHeight,
      );
    }
    const geometry = shape.geometry;
    const centre = {
      x: geometry.x + geometry.width / 2,
      y: geometry.y + geometry.height / 2,
    };
    return {
      ...geometry,
      rotation: shapeRotationFromPointer(
        centre,
        interaction.start,
        interaction.current,
        interaction.startRotation,
        interaction.snap ? 15 : 0,
      ),
    };
  }

  async function createFromInteraction(
    interaction: Extract<Interaction, { kind: "create" }>,
  ) {
    const rect = shapeRectFromDrag(
      interaction.anchor,
      interaction.current,
      pageWidth,
      pageHeight,
    );
    if (!rect) {
      // A click or a sliver: keep the tool active and leave the document
      // untouched, matching the backend's rejection of zero-sized shapes.
      return;
    }
    const pendingStyle = style;
    setInteraction(null);
    setBusy(true);
    setNotice(null);
    try {
      const result = await createShapeAnnotation(documentId, {
        pageIndex,
        kind: interaction.shapeKind,
        geometry: { ...rect, rotation: 0 },
        style: pendingStyle,
      });
      notifyDocumentChanged({
        documentId,
        pageIndex,
        source: "shape",
      });
      onSelectAnnotation?.(result.annotation);
      setTool(null);
    } catch (error) {
      setNotice({ kind: "error", text: toCommandError(error).message });
    } finally {
      setBusy(false);
    }
  }

  async function commitGeometry(
    annotation: AnnotationView,
    geometry: ShapeGeometry,
  ) {
    if (!annotation.shape || geometryEquals(annotation.shape.geometry, geometry)) {
      setInteraction(null);
      return;
    }
    setInteraction(null);
    setBusy(true);
    setNotice(null);
    try {
      const result = await updateShapeGeometry(
        documentId,
        annotation.name,
        geometry,
      );
      notifyDocumentChanged({
        documentId,
        pageIndex,
        source: "shape",
      });
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
    const geometry = previewGeometryFor(interaction);
    if (geometry) {
      void commitGeometry(interaction.annotation, geometry);
    } else {
      setInteraction(null);
    }
  }

  const selected =
    selectedAnnotation?.pageIndex === pageIndex && selectedAnnotation.shape
      ? selectedAnnotation
      : null;
  const previewGeometry = interaction ? previewGeometryFor(interaction) : null;
  const previewAnnotation =
    interaction && interaction.kind !== "create" ? interaction.annotation : null;
  const chromeGeometry =
    previewGeometry ?? selected?.shape?.geometry ?? null;
  const chromeKind =
    previewAnnotation?.shape?.kind ?? selected?.shape?.kind ?? null;

  const createPreview =
    interaction?.kind === "create" && tool
      ? shapeRectFromDrag(
          interaction.anchor,
          interaction.current,
          pageWidth,
          pageHeight,
        )
      : null;

  const pageShapes = useMemo(
    () =>
      annotations.filter(
        (annotation) =>
          annotation.pageIndex === pageIndex && annotation.shape !== null,
      ),
    [annotations, pageIndex],
  );

  const scaleSafe = scale > 0 ? scale : 1;
  let rotateHandle: Point | null = null;
  let resizeHandle: Point | null = null;
  if (chromeGeometry && chromeKind) {
    const centre = {
      x: chromeGeometry.x + chromeGeometry.width / 2,
      y: chromeGeometry.y + chromeGeometry.height / 2,
    };
    resizeHandle = rotatePoint(
      chromeGeometry.x + chromeGeometry.width,
      chromeGeometry.y + chromeGeometry.height,
      centre.x,
      centre.y,
      chromeGeometry.rotation,
    );
    rotateHandle = rotatePoint(
      centre.x,
      chromeGeometry.y - ROTATE_HANDLE_OFFSET / scaleSafe,
      centre.x,
      centre.y,
      chromeGeometry.rotation,
    );
  }

  const percent = (value: number, total: number): string =>
    `${(value / total) * 100}%`;
  const handleStyle = (point: Point): CSSProperties => ({
    left: percent(point.x, pageWidth),
    top: percent(point.y, pageHeight),
  });

  return (
    <div
      ref={layerRef}
      className={`shape-layer${tool ? " shape-layer--drawing" : ""}`}
      aria-label="形状层"
    >
      <svg
        className="shape-layer-svg"
        viewBox={`0 0 ${pageWidth} ${pageHeight}`}
        preserveAspectRatio="none"
        aria-hidden="true"
      >
        {pageShapes.map((annotation) => {
          const shape = annotation.shape;
          if (!shape) {
            return null;
          }
          return (
            <HitShape
              key={annotation.name}
              kind={shape.kind}
              geometry={shape.geometry}
              testId={`shape-hit-${annotation.name}`}
              onPointerDown={(event) => startMove(event, annotation)}
              onPointerMove={moveInteraction}
              onPointerUp={endInteraction}
              onPointerCancel={endInteraction}
            />
          );
        })}

        {previewAnnotation?.shape && previewGeometry && (
          <ShapeElement
            kind={previewAnnotation.shape.kind}
            geometry={previewGeometry}
            style={previewAnnotation.shape.style}
            className="shape-transform-preview"
            testId="shape-transform-preview"
          />
        )}

        {createPreview && interaction?.kind === "create" && (
          <ShapeElement
            kind={interaction.shapeKind}
            geometry={{ ...createPreview, rotation: 0 }}
            style={style}
            className="shape-create-preview"
            testId="shape-create-preview"
          />
        )}

        {chromeGeometry && chromeKind && (
          <ShapeElement
            kind={chromeKind}
            geometry={chromeGeometry}
            style={{
              fillColor: null,
              strokeColor: "#3f6ad8",
              opacity: 100,
              lineWidth: 1.5,
            }}
            className="shape-selection-outline"
            testId="shape-selection-outline"
            strokeDasharray="6 4"
            vectorEffect="non-scaling-stroke"
          />
        )}
      </svg>

      {tool && (
        <div
          className="shape-draw-surface"
          data-testid="shape-draw-surface"
          onPointerDown={startCreate}
          onPointerMove={moveInteraction}
          onPointerUp={endInteraction}
          onPointerCancel={endInteraction}
          onContextMenu={(event) => event.preventDefault()}
        />
      )}

      {canEdit && (
        <ToolShelf
          defaultToolId="square"
          tools={[
            {
              id: "square",
              label: "矩形",
              icon: "shape",
              groups: ["shapes"],
              active: tool === "square",
              disabled: !drawable || busy,
              title: !drawable
                ? "至少需要填充或描边中的一项"
                : "拖动绘制矩形",
              order: 1,
              onActivate: () => {
                setNotice(null);
                setInteraction(null);
                setTool((current) => (current === "square" ? null : "square"));
              },
            },
            {
              id: "circle",
              label: "椭圆",
              icon: "circle",
              groups: ["shapes"],
              active: tool === "circle",
              disabled: !drawable || busy,
              title: !drawable
                ? "至少需要填充或描边中的一项"
                : "拖动绘制椭圆",
              order: 2,
              onActivate: () => {
                setNotice(null);
                setInteraction(null);
                setTool((current) => (current === "circle" ? null : "circle"));
              },
            },
            {
              id: "comment-shape",
              label: "添加形状",
              icon: "shape",
              groups: ["comment"],
              active: tool === "square",
              disabled: !drawable || busy,
              title: !drawable
                ? "至少需要填充或描边中的一项"
                : "拖动绘制矩形",
              order: 4,
              onActivate: () => {
                setNotice(null);
                setInteraction(null);
                setTool((current) => (current === "square" ? null : "square"));
              },
            },
          ] satisfies ToolShelfTool[]}
          panelTitle="形状"
          panelOpen={tool !== null}
        >
        <div
          className="shape-toolbar"
          role="toolbar"
          aria-label="形状工具"
          onPointerDown={(event) => event.stopPropagation()}
        >
          <label className="shape-toolbar-control">
            <input
              type="checkbox"
              aria-label="填充"
              checked={fillEnabled}
              disabled={busy}
              onChange={(event) => setFillEnabled(event.target.checked)}
            />
            填充
          </label>
          <input
            type="color"
            aria-label="填充颜色"
            value={fillColor}
            disabled={!fillEnabled || busy}
            onChange={(event) => setFillColor(event.target.value)}
          />
          <label className="shape-toolbar-control">
            <input
              type="checkbox"
              aria-label="描边"
              checked={strokeEnabled}
              disabled={busy}
              onChange={(event) => setStrokeEnabled(event.target.checked)}
            />
            描边
          </label>
          <input
            type="color"
            aria-label="描边颜色"
            value={strokeColor}
            disabled={!strokeEnabled || busy}
            onChange={(event) => setStrokeColor(event.target.value)}
          />
          <label className="shape-toolbar-control">
            线宽
            <input
              type="number"
              aria-label="线宽"
              min={MIN_LINE_WIDTH}
              max={MAX_LINE_WIDTH}
              step={0.5}
              value={lineWidth}
              disabled={!strokeEnabled || busy}
              onChange={(event) => {
                const next = Number.parseFloat(event.target.value);
                if (Number.isFinite(next)) {
                  setLineWidth(
                    Math.min(Math.max(next, MIN_LINE_WIDTH), MAX_LINE_WIDTH),
                  );
                }
              }}
            />
          </label>
          <label className="shape-toolbar-control">
            透明度
            <input
              type="range"
              aria-label="形状透明度"
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
          {!drawable && (
            <span className="shape-toolbar-warning" role="status">
              至少需要填充或描边中的一项
            </span>
          )}
          {notice && (
            <span
              className={`text-edit-notice text-edit-notice--${notice.kind}`}
              role={notice.kind === "error" ? "alert" : "status"}
            >
              {notice.text}
            </span>
          )}
        </div>
        </ToolShelf>
      )}

      {canEdit &&
        !busy &&
        !tool &&
        selected &&
        resizeHandle &&
        rotateHandle && (
          <>
            <button
              type="button"
              className="shape-handle shape-handle--resize"
              aria-label="调整形状大小"
              data-testid="shape-resize-handle"
              style={handleStyle(resizeHandle)}
              onPointerDown={(event) => startResize(event, selected)}
              onPointerMove={moveInteraction}
              onPointerUp={endInteraction}
              onPointerCancel={endInteraction}
            />
            <button
              type="button"
              className="shape-handle shape-handle--rotate"
              aria-label="旋转形状"
              data-testid="shape-rotate-handle"
              style={handleStyle(rotateHandle)}
              onPointerDown={(event) => startRotate(event, selected)}
              onPointerMove={moveInteraction}
              onPointerUp={endInteraction}
              onPointerCancel={endInteraction}
            />
          </>
        )}
    </div>
  );
}

function ShapeElement({
  kind,
  geometry,
  style,
  className,
  testId,
  strokeDasharray,
  vectorEffect,
}: {
  kind: ShapeKind;
  geometry: ShapeGeometryLike;
  style: ShapeStyle;
  className: string;
  testId: string;
  strokeDasharray?: string;
  vectorEffect?: "non-scaling-stroke";
}) {
  const rect = outlineGeometry(geometry);
  const transform = shapeTransform(rect, geometry.rotation);
  const common = {
    className,
    "data-testid": testId,
    "data-kind": kind,
    fill: style.fillColor ?? "none",
    stroke: style.strokeColor ?? "none",
    strokeWidth: style.strokeColor ? style.lineWidth : 0,
    opacity: style.opacity / 100,
    strokeDasharray,
    vectorEffect,
  };
  return kind === "circle" ? (
    <ellipse
      cx={rect.x + rect.width / 2}
      cy={rect.y + rect.height / 2}
      rx={rect.width / 2}
      ry={rect.height / 2}
      transform={transform}
      {...common}
    />
  ) : (
    <rect
      x={rect.x}
      y={rect.y}
      width={rect.width}
      height={rect.height}
      transform={transform}
      {...common}
    />
  );
}

function HitShape({
  kind,
  geometry,
  testId,
  onPointerDown,
  onPointerMove,
  onPointerUp,
  onPointerCancel,
}: {
  kind: ShapeKind;
  geometry: ShapeGeometryLike;
  testId: string;
  onPointerDown: (event: ReactPointerEvent<SVGElement>) => void;
  onPointerMove: (event: ReactPointerEvent<SVGElement>) => void;
  onPointerUp: (event: ReactPointerEvent<SVGElement>) => void;
  onPointerCancel: (event: ReactPointerEvent<SVGElement>) => void;
}) {
  const rect = outlineGeometry(geometry);
  const transform = shapeTransform(rect, geometry.rotation);
  const common = {
    className: "shape-hit",
    "data-testid": testId,
    fill: "transparent",
    stroke: "none",
    onPointerDown,
    onPointerMove,
    onPointerUp,
    onPointerCancel,
  };
  return kind === "circle" ? (
    <ellipse
      cx={rect.x + rect.width / 2}
      cy={rect.y + rect.height / 2}
      rx={rect.width / 2}
      ry={rect.height / 2}
      transform={transform}
      {...common}
    />
  ) : (
    <rect
      x={rect.x}
      y={rect.y}
      width={rect.width}
      height={rect.height}
      transform={transform}
      {...common}
    />
  );
}

/** Used by the annotation list label and tests; keeps the kind vocabulary here. */
export function shapeKindLabel(kind: ShapeKind): string {
  return KIND_LABELS[kind];
}
