import {
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
} from "react";

import {
  createLineObject,
  deleteLineObject,
  pageLineObjects,
  toCommandError,
  transformLineObject,
  updateLineStyle,
  type LineGeometry,
  type LineKind,
  type LineObjectView,
  type LineStyle,
} from "../lib/backend";
import { notifyDocumentChanged, onDocumentChanged } from "../lib/documentEvents";
import {
  arrowHeadPoints,
  lineAngle,
  lineFromDrag,
  lineLength,
  lineMidpoint,
  lineRotationFromPointer,
  moveLine,
  moveLineEndpoint,
  rotateLine,
  MIN_LINE_LENGTH,
  type LineEndpoint,
  type Point,
} from "../lib/lineGeometry";

/** Defaults chosen to be visible on both light and dark pages. */
const DEFAULT_STROKE = "#000000";
const DEFAULT_OPACITY = 100;
const DEFAULT_LINE_WIDTH = 2;
const MIN_LINE_WIDTH = 0.5;
const MAX_LINE_WIDTH = 100;
/** Screen-space distance from the midpoint to the rotate handle, in points. */
const ROTATE_HANDLE_OFFSET = 18;
/** Pointer travel below this is a click, not a transform. */
const MOVE_SLOP = 0.5;
const KIND_LABELS: Record<LineKind, string> = {
  line: "直线",
  arrow: "箭头",
};
/** The shortest segment the backend accepts; shorter drags leave no object. */
const TOO_SHORT_MESSAGE = "直线至少需要 2 pt";
const PAGE_CONTENT_NOTE =
  "线条是页面内容，不是注释，不会出现在注释列表；删除后可通过撤销恢复。";

/** Chinese copy for command failures the overlay can explain specifically. */
const LINE_ERROR_MESSAGES: Partial<Record<string, string>> = {
  permission_denied: "文档所有者限制了编辑权限。",
  document_not_found: "文档已关闭，请重新打开。",
  page_out_of_range: "页码无效。",
  invalid_argument: "线条位置或样式无效。",
  render_failed: "无法绘制线条。",
  internal: "线条操作失败，请重试。",
};

function lineErrorText(error: unknown): string {
  const commandError = toCommandError(error);
  return LINE_ERROR_MESSAGES[commandError.code] ?? "线条操作失败，请重试。";
}

type Tool = LineKind | null;
type Notice = { kind: "info" | "error"; text: string };

type Interaction =
  | {
      kind: "create";
      lineKind: LineKind;
      anchor: Point;
      current: Point;
    }
  | {
      kind: "move";
      line: LineObjectView;
      start: Point;
      current: Point;
    }
  | {
      kind: "endpoint";
      line: LineObjectView;
      endpoint: LineEndpoint;
      current: Point;
    }
  | {
      kind: "rotate";
      line: LineObjectView;
      start: Point;
      current: Point;
      snap: boolean;
    };

export interface LineLayerProps {
  documentId: string;
  pageIndex: number;
  /** Display-space page size, matching the other overlays. */
  pageWidth: number;
  pageHeight: number;
  /** Page render scale, used to keep handles a constant screen size. */
  scale?: number;
  /** Permission Restrictions deny every editing capability. */
  readOnly?: boolean;
  /** The PDF security handler allows changing document content. */
  canModifyContent?: boolean;
}

function geometryEquals(left: LineGeometry, right: LineGeometry): boolean {
  return (
    Math.abs(left.x1 - right.x1) < MOVE_SLOP &&
    Math.abs(left.y1 - right.y1) < MOVE_SLOP &&
    Math.abs(left.x2 - right.x2) < MOVE_SLOP &&
    Math.abs(left.y2 - right.y2) < MOVE_SLOP
  );
}

function styleEquals(left: LineStyle, right: LineStyle): boolean {
  return (
    left.strokeColor === right.strokeColor &&
    left.opacity === right.opacity &&
    Math.abs(left.lineWidth - right.lineWidth) < 0.001
  );
}

/**
 * Canvas overlay for straight lines and arrows.
 *
 * Lines are page *content* objects, not annotations: they never enter the
 * annotation list and deleting one is an undoable content edit. The toolbar
 * draws a live SVG preview with the same geometry, stroke and arrow-head
 * formula the backend serializes into the page content stream. Selecting a
 * line exposes endpoint handles (resize/reshape), a rotate handle and a
 * stroke-only property panel matching the rectangle/ellipse panel.
 */
export function LineLayer({
  documentId,
  pageIndex,
  pageWidth,
  pageHeight,
  scale = 1,
  readOnly = false,
  canModifyContent = false,
}: LineLayerProps) {
  const [lines, setLines] = useState<LineObjectView[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [tool, setTool] = useState<Tool>(null);
  const [strokeColor, setStrokeColor] = useState(DEFAULT_STROKE);
  const [lineWidth, setLineWidth] = useState(DEFAULT_LINE_WIDTH);
  const [opacity, setOpacity] = useState(DEFAULT_OPACITY);
  const [draftStroke, setDraftStroke] = useState(DEFAULT_STROKE);
  const [draftLineWidth, setDraftLineWidth] = useState(DEFAULT_LINE_WIDTH);
  const [draftOpacity, setDraftOpacity] = useState(DEFAULT_OPACITY);
  const [interaction, setInteraction] = useState<Interaction | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const layerRef = useRef<HTMLDivElement>(null);
  const pageKey = useRef<string | null>(null);

  const canEdit = canModifyContent && !readOnly;
  const style: LineStyle = { strokeColor, opacity, lineWidth };

  // Load this session's line objects. Undo, redo and other tools announce a
  // page change; this layer refetches unless it made the change itself.
  useEffect(() => {
    let cancelled = false;
    const key = `${documentId}:${pageIndex}`;
    if (pageKey.current !== key) {
      pageKey.current = key;
      setLines([]);
      setSelectedId(null);
      setInteraction(null);
      setNotice(null);
    }
    pageLineObjects(documentId, pageIndex)
      .then((list) => {
        if (cancelled) {
          return;
        }
        setLines(list);
        setSelectedId((current) =>
          current !== null && list.some((entry) => entry.lineId === current)
            ? current
            : null,
        );
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setLines([]);
          setSelectedId(null);
          setNotice({ kind: "error", text: lineErrorText(error) });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [documentId, pageIndex, reloadKey]);

  useEffect(
    () =>
      onDocumentChanged((detail) => {
        if (
          detail.documentId === documentId &&
          detail.pageIndex === pageIndex &&
          detail.source !== "line"
        ) {
          setReloadKey((value) => value + 1);
        }
      }),
    [documentId, pageIndex],
  );

  useEffect(() => {
    if (!canEdit) {
      setTool(null);
      setInteraction(null);
    }
  }, [canEdit]);

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

  const selected =
    selectedId !== null
      ? (lines.find((entry) => entry.lineId === selectedId) ?? null)
      : null;

  const selectedStyle = selected?.style ?? null;

  useEffect(() => {
    setDraftStroke(selectedStyle?.strokeColor ?? DEFAULT_STROKE);
    setDraftLineWidth(selectedStyle?.lineWidth ?? DEFAULT_LINE_WIDTH);
    setDraftOpacity(selectedStyle?.opacity ?? DEFAULT_OPACITY);
  }, [
    selected?.lineId,
    selectedStyle?.strokeColor,
    selectedStyle?.lineWidth,
    selectedStyle?.opacity,
  ]);

  const draftStyle: LineStyle | null = selected
    ? {
        strokeColor: draftStroke,
        opacity: draftOpacity,
        lineWidth: draftLineWidth,
      }
    : null;
  const styleChanged =
    selected !== null &&
    selectedStyle !== null &&
    draftStyle !== null &&
    !styleEquals(draftStyle, selectedStyle);

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
      x: ((event.clientX - box.left) / box.width) * pageWidth,
      y: ((event.clientY - box.top) / box.height) * pageHeight,
    };
  }

  function capturePointer(event: ReactPointerEvent<Element>) {
    if (typeof event.currentTarget.setPointerCapture === "function") {
      try {
        event.currentTarget.setPointerCapture(event.pointerId);
      } catch {
        // Pointer capture is unavailable in older webviews; bubbled events
        // inside the overlay are still enough for a normal drag.
      }
    }
  }

  function startCreate(event: ReactPointerEvent<HTMLDivElement>) {
    if (!canEdit || busy || tool === null || event.button !== 0) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    capturePointer(event);
    setNotice(null);
    setInteraction({
      kind: "create",
      lineKind: tool,
      anchor: point,
      current: point,
    });
  }

  function startMove(
    event: ReactPointerEvent<SVGElement>,
    line: LineObjectView,
  ) {
    event.stopPropagation();
    setSelectedId(line.lineId);
    if (!canEdit || busy || tool || event.button !== 0) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    event.preventDefault();
    capturePointer(event);
    setNotice(null);
    setInteraction({ kind: "move", line, start: point, current: point });
  }

  function startEndpoint(
    event: ReactPointerEvent<HTMLButtonElement>,
    line: LineObjectView,
    endpoint: LineEndpoint,
  ) {
    if (!canEdit || busy) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    capturePointer(event);
    setNotice(null);
    setInteraction({ kind: "endpoint", line, endpoint, current: point });
  }

  function startRotate(
    event: ReactPointerEvent<HTMLButtonElement>,
    line: LineObjectView,
  ) {
    if (!canEdit || busy) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    capturePointer(event);
    setNotice(null);
    setInteraction({
      kind: "rotate",
      line,
      start: point,
      current: point,
      snap: event.shiftKey,
    });
  }

  function moveInteraction(event: ReactPointerEvent<Element>) {
    if (!interaction) {
      return;
    }
    const point = pointFromEvent(event) ?? interaction.current;
    setInteraction(
      interaction.kind === "rotate"
        ? { ...interaction, current: point, snap: event.shiftKey }
        : { ...interaction, current: point },
    );
  }

  function previewGeometryFor(interaction: Interaction): LineGeometry | null {
    if (interaction.kind === "create") {
      return lineFromDrag(
        interaction.anchor,
        interaction.current,
        pageWidth,
        pageHeight,
      );
    }
    if (interaction.kind === "move") {
      return moveLine(
        interaction.line.geometry,
        {
          x: interaction.current.x - interaction.start.x,
          y: interaction.current.y - interaction.start.y,
        },
        pageWidth,
        pageHeight,
      );
    }
    if (interaction.kind === "endpoint") {
      return moveLineEndpoint(
        interaction.line.geometry,
        interaction.endpoint,
        interaction.current,
        pageWidth,
        pageHeight,
      );
    }
    return rotateLine(
      interaction.line.geometry,
      lineRotationFromPointer(
        interaction.line.geometry,
        interaction.start,
        interaction.current,
        interaction.snap ? 15 : 0,
      ),
    );
  }

  async function createFromInteraction(
    interaction: Extract<Interaction, { kind: "create" }>,
  ) {
    const geometry = lineFromDrag(
      interaction.anchor,
      interaction.current,
      pageWidth,
      pageHeight,
    );
    if (!geometry) {
      // A click or a sliver: keep the tool active and leave the document
      // untouched, matching the backend's minimum-length rejection.
      return;
    }
    const pendingStyle = style;
    setBusy(true);
    setNotice(null);
    try {
      const result = await createLineObject(documentId, {
        pageIndex,
        kind: interaction.lineKind,
        geometry,
        style: pendingStyle,
      });
      setLines((list) => [
        ...list.filter((entry) => entry.lineId !== result.line.lineId),
        result.line,
      ]);
      setSelectedId(result.line.lineId);
      setTool(null);
      notifyDocumentChanged({ documentId, pageIndex, source: "line" });
    } catch (error) {
      setNotice({ kind: "error", text: lineErrorText(error) });
    } finally {
      setBusy(false);
    }
  }

  async function commitTransform(line: LineObjectView, geometry: LineGeometry) {
    if (geometryEquals(line.geometry, geometry)) {
      return;
    }
    setBusy(true);
    setNotice(null);
    try {
      const result = await transformLineObject(
        documentId,
        pageIndex,
        line.lineId,
        { geometry },
      );
      setLines((list) =>
        list.map((entry) =>
          entry.lineId === result.line.lineId ? result.line : entry,
        ),
      );
      notifyDocumentChanged({ documentId, pageIndex, source: "line" });
    } catch (error) {
      setNotice({ kind: "error", text: lineErrorText(error) });
    } finally {
      setBusy(false);
    }
  }

  function settleInteraction(event: ReactPointerEvent<Element>) {
    if (!interaction) {
      return;
    }
    const point = pointFromEvent(event) ?? interaction.current;
    const settled: Interaction =
      interaction.kind === "create"
        ? { ...interaction, current: point }
        : interaction.kind === "rotate"
          ? { ...interaction, current: point, snap: event.shiftKey }
          : { ...interaction, current: point };
    setInteraction(null);
    if (settled.kind === "create") {
      void createFromInteraction(settled);
      return;
    }
    const geometry = previewGeometryFor(settled);
    if (!geometry) {
      return;
    }
    if (lineLength(geometry) < MIN_LINE_LENGTH) {
      setNotice({ kind: "info", text: TOO_SHORT_MESSAGE });
      return;
    }
    void commitTransform(settled.line, geometry);
  }

  async function applyStyle() {
    if (!selected || !draftStyle || !styleChanged || busy) {
      return;
    }
    setBusy(true);
    setNotice(null);
    try {
      const result = await updateLineStyle(
        documentId,
        pageIndex,
        selected.lineId,
        draftStyle,
      );
      setLines((list) =>
        list.map((entry) =>
          entry.lineId === result.line.lineId ? result.line : entry,
        ),
      );
      notifyDocumentChanged({ documentId, pageIndex, source: "line" });
    } catch (error) {
      setNotice({ kind: "error", text: lineErrorText(error) });
    } finally {
      setBusy(false);
    }
  }

  async function removeSelected() {
    if (!selected || busy) {
      return;
    }
    setBusy(true);
    setNotice(null);
    try {
      await deleteLineObject(documentId, pageIndex, selected.lineId);
      setLines((list) => list.filter((entry) => entry.lineId !== selected.lineId));
      setSelectedId(null);
      setInteraction(null);
      notifyDocumentChanged({ documentId, pageIndex, source: "line" });
    } catch (error) {
      setNotice({ kind: "error", text: lineErrorText(error) });
    } finally {
      setBusy(false);
    }
  }

  const selectedPreview =
    selected &&
    interaction !== null &&
    interaction.kind !== "create" &&
    interaction.line.lineId === selected.lineId
      ? previewGeometryFor(interaction)
      : null;
  const chromeGeometry = selectedPreview ?? selected?.geometry ?? null;

  let rotateHandle: Point | null = null;
  if (chromeGeometry) {
    const midpoint = lineMidpoint(chromeGeometry);
    const length = Math.max(lineLength(chromeGeometry), 1);
    // Screen-space normal pointing above a left-to-right line.
    const normalX = (chromeGeometry.y2 - chromeGeometry.y1) / length;
    const normalY = -(chromeGeometry.x2 - chromeGeometry.x1) / length;
    const offset = ROTATE_HANDLE_OFFSET / (scale > 0 ? scale : 1);
    rotateHandle = {
      x: midpoint.x + normalX * offset,
      y: midpoint.y + normalY * offset,
    };
  }

  const createInteraction =
    interaction?.kind === "create" ? interaction : null;
  const createGeometry = createInteraction
    ? lineFromDrag(
        createInteraction.anchor,
        createInteraction.current,
        pageWidth,
        pageHeight,
      )
    : null;

  const percent = (value: number, total: number): string =>
    `${(value / total) * 100}%`;
  const pointStyle = (point: Point): CSSProperties => ({
    left: percent(point.x, pageWidth),
    top: percent(point.y, pageHeight),
  });

  return (
    <div
      ref={layerRef}
      className={`line-layer${tool ? " line-layer--drawing" : ""}`}
      role="group"
      aria-label="直线与箭头层"
      data-testid="line-layer"
    >
      <svg
        className="line-layer-svg"
        viewBox={`0 0 ${pageWidth} ${pageHeight}`}
        preserveAspectRatio="none"
        aria-hidden="true"
      >
        {lines.map((line) => {
          const preview =
            interaction !== null &&
            interaction.kind !== "create" &&
            interaction.line.lineId === line.lineId
              ? previewGeometryFor(interaction)
              : null;
          const geometry = preview ?? line.geometry;
          return (
            <g key={line.lineId}>
              <LineGlyph
                kind={line.kind}
                geometry={geometry}
                style={line.style}
                className="line-object"
                testId={`line-object-${line.lineId}`}
              />
              <HitLine
                geometry={geometry}
                lineWidth={line.style.lineWidth}
                testId={`line-hit-${line.lineId}`}
                onPointerDown={(event) => startMove(event, line)}
                onPointerMove={moveInteraction}
                onPointerUp={settleInteraction}
                onPointerCancel={() => setInteraction(null)}
              />
            </g>
          );
        })}

        {selected && chromeGeometry && !tool && (
          <line
            className="line-selection-outline"
            data-testid="line-selection-outline"
            x1={chromeGeometry.x1}
            y1={chromeGeometry.y1}
            x2={chromeGeometry.x2}
            y2={chromeGeometry.y2}
            stroke="#3f6ad8"
            strokeWidth={1.5}
            strokeDasharray="6 4"
            vectorEffect="non-scaling-stroke"
            fill="none"
          />
        )}

        {createGeometry && createInteraction && (
          <LineGlyph
            kind={createInteraction.lineKind}
            geometry={createGeometry}
            style={style}
            className="line-create-preview"
            testId="line-create-preview"
          />
        )}
      </svg>

      {tool && (
        <div
          className="line-draw-surface"
          data-testid="line-draw-surface"
          onPointerDown={startCreate}
          onPointerMove={moveInteraction}
          onPointerUp={settleInteraction}
          onPointerCancel={() => setInteraction(null)}
          onContextMenu={(event) => event.preventDefault()}
        />
      )}

      {canEdit && (
        <div
          className="line-controls"
          onPointerDown={(event) => event.stopPropagation()}
        >
          <div className="line-toolbar" role="toolbar" aria-label="直线与箭头工具">
            <span className="line-toolbar-title">线条</span>
            <button
              type="button"
              aria-pressed={tool === "line"}
              disabled={busy}
              title="拖动绘制直线"
              onClick={() => {
                setNotice(null);
                setInteraction(null);
                setTool((current) => (current === "line" ? null : "line"));
              }}
            >
              直线
            </button>
            <button
              type="button"
              aria-pressed={tool === "arrow"}
              disabled={busy}
              title="拖动绘制箭头"
              onClick={() => {
                setNotice(null);
                setInteraction(null);
                setTool((current) => (current === "arrow" ? null : "arrow"));
              }}
            >
              箭头
            </button>
            <label className="line-toolbar-control">
              <span>颜色</span>
              <input
                type="color"
                aria-label="线条颜色"
                value={strokeColor}
                disabled={busy}
                onChange={(event) => setStrokeColor(event.target.value)}
              />
            </label>
            <label className="line-toolbar-control">
              线宽
              <input
                type="number"
                aria-label="线条线宽"
                min={MIN_LINE_WIDTH}
                max={MAX_LINE_WIDTH}
                step={0.5}
                value={lineWidth}
                disabled={busy}
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
            <label className="line-toolbar-control">
              透明度
              <input
                type="range"
                aria-label="线条透明度"
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
                className={`line-notice line-notice--${notice.kind}`}
                role={notice.kind === "error" ? "alert" : "status"}
              >
                {notice.text}
              </span>
            )}
          </div>

          {selected && !tool && (
            <div className="line-properties" aria-label="线条属性">
              <p className="line-meta">
                {KIND_LABELS[selected.kind]} · 长度{" "}
                {Math.round(lineLength(selected.geometry))} pt · 角度{" "}
                {Math.round(lineAngle(selected.geometry))}°
              </p>
              <label className="line-toolbar-control">
                <span>描边颜色</span>
                <input
                  type="color"
                  aria-label="线条描边颜色"
                  value={draftStroke}
                  disabled={busy}
                  onChange={(event) => setDraftStroke(event.target.value)}
                />
              </label>
              <label className="line-toolbar-control">
                <span>线宽</span>
                <input
                  type="number"
                  aria-label="线条描边宽度"
                  min={MIN_LINE_WIDTH}
                  max={MAX_LINE_WIDTH}
                  step={0.5}
                  value={draftLineWidth}
                  disabled={busy}
                  onChange={(event) => {
                    const next = Number.parseFloat(event.target.value);
                    if (Number.isFinite(next)) {
                      setDraftLineWidth(
                        Math.min(Math.max(next, MIN_LINE_WIDTH), MAX_LINE_WIDTH),
                      );
                    }
                  }}
                />
              </label>
              <label className="line-toolbar-control">
                <span>透明度</span>
                <input
                  type="range"
                  aria-label="线条描边透明度"
                  min={0}
                  max={100}
                  step={1}
                  value={draftOpacity}
                  disabled={busy}
                  onChange={(event) =>
                    setDraftOpacity(Number.parseInt(event.target.value, 10))
                  }
                />
                <span>{draftOpacity}%</span>
              </label>
              <p className="line-note">{PAGE_CONTENT_NOTE}</p>
              <div className="line-actions">
                <button
                  type="button"
                  disabled={!styleChanged || busy}
                  onClick={() => void applyStyle()}
                >
                  应用样式
                </button>
                <button
                  type="button"
                  className="line-delete"
                  disabled={busy}
                  onClick={() => void removeSelected()}
                >
                  删除线条
                </button>
              </div>
            </div>
          )}
        </div>
      )}

      {canEdit && !busy && !tool && selected && chromeGeometry && rotateHandle && (
        <>
          <button
            type="button"
            className="line-handle line-handle--start"
            aria-label="调整直线起点"
            data-testid="line-endpoint-start"
            style={pointStyle({ x: chromeGeometry.x1, y: chromeGeometry.y1 })}
            onPointerDown={(event) => startEndpoint(event, selected, "start")}
            onPointerMove={moveInteraction}
            onPointerUp={settleInteraction}
            onPointerCancel={() => setInteraction(null)}
          />
          <button
            type="button"
            className="line-handle line-handle--end"
            aria-label="调整直线终点"
            data-testid="line-endpoint-end"
            style={pointStyle({ x: chromeGeometry.x2, y: chromeGeometry.y2 })}
            onPointerDown={(event) => startEndpoint(event, selected, "end")}
            onPointerMove={moveInteraction}
            onPointerUp={settleInteraction}
            onPointerCancel={() => setInteraction(null)}
          />
          <button
            type="button"
            className="line-handle line-handle--rotate"
            aria-label="旋转直线"
            data-testid="line-rotate-handle"
            style={pointStyle(rotateHandle)}
            onPointerDown={(event) => startRotate(event, selected)}
            onPointerMove={moveInteraction}
            onPointerUp={settleInteraction}
            onPointerCancel={() => setInteraction(null)}
          />
        </>
      )}
    </div>
  );
}

function LineGlyph({
  kind,
  geometry,
  style,
  className,
  testId,
}: {
  kind: LineKind;
  geometry: LineGeometry;
  style: LineStyle;
  className: string;
  testId: string;
}) {
  const opacityValue = style.opacity / 100;
  return (
    <g
      className={className}
      data-testid={testId}
      data-kind={kind}
      opacity={opacityValue}
    >
      <line
        className="line-shaft"
        x1={geometry.x1}
        y1={geometry.y1}
        x2={geometry.x2}
        y2={geometry.y2}
        stroke={style.strokeColor}
        strokeWidth={style.lineWidth}
        strokeLinecap="round"
        fill="none"
      />
      {kind === "arrow" && (
        <polygon
          className="line-arrow-head"
          data-testid={`${testId}-head`}
          points={arrowHeadPoints(geometry, style.lineWidth)
            .map((point) => `${point.x},${point.y}`)
            .join(" ")}
          fill={style.strokeColor}
          stroke={style.strokeColor}
          strokeWidth={style.lineWidth}
        />
      )}
    </g>
  );
}

function HitLine({
  geometry,
  lineWidth,
  testId,
  onPointerDown,
  onPointerMove,
  onPointerUp,
  onPointerCancel,
}: {
  geometry: LineGeometry;
  lineWidth: number;
  testId: string;
  onPointerDown: (event: ReactPointerEvent<SVGElement>) => void;
  onPointerMove: (event: ReactPointerEvent<SVGElement>) => void;
  onPointerUp: (event: ReactPointerEvent<SVGElement>) => void;
  onPointerCancel: (event: ReactPointerEvent<SVGElement>) => void;
}) {
  return (
    <line
      className="line-hit"
      data-testid={testId}
      x1={geometry.x1}
      y1={geometry.y1}
      x2={geometry.x2}
      y2={geometry.y2}
      stroke="transparent"
      strokeWidth={Math.max(lineWidth, 12)}
      strokeLinecap="round"
      fill="none"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerCancel}
    />
  );
}

/** Used by tests and future panels; keeps the kind vocabulary here. */
export function lineKindLabel(kind: LineKind): string {
  return KIND_LABELS[kind];
}
