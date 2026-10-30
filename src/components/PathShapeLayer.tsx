import { useRef, useState, type PointerEvent } from "react";
import {
  createInkAnnotation,
  toCommandError,
  type InkPoint,
} from "../lib/backend";
import { notifyDocumentChanged } from "../lib/documentEvents";
import { ToolShelf, type ToolShelfTool } from "./ToolShelf";

type PathKind = "arc" | "polygon" | "cloud" | "polyline";
const names: Record<PathKind, string> = {
  arc: "弧",
  polygon: "多边形",
  cloud: "云",
  polyline: "折线",
};
const icons: Record<PathKind, ToolShelfTool["icon"]> = {
  arc: "arc",
  polygon: "polygon",
  cloud: "cloud",
  polyline: "polyline",
};
const orders = { arc: 3, polygon: 4, cloud: 5, polyline: 7 };
export function shapeStroke(
  kind: PathKind,
  start: InkPoint,
  end: InkPoint,
): InkPoint[] {
  const left = Math.min(start.x, end.x),
    top = Math.min(start.y, end.y),
    width = Math.abs(end.x - start.x),
    height = Math.abs(end.y - start.y);
  const cx = left + width / 2,
    cy = top + height / 2;
  if (kind === "arc")
    return Array.from({ length: 49 }, (_, i) => {
      const a = Math.PI + (i / 48) * Math.PI;
      return {
        x: cx + (width / 2) * Math.cos(a),
        y: top + height + height * Math.sin(a),
      };
    });
  return Array.from({ length: 193 }, (_, i) => {
    const a = (i / 192) * Math.PI * 2;
    const ripple = 1 + 0.085 * Math.sin(a * 12);
    return {
      x: cx + (width / 2) * Math.cos(a) * ripple,
      y: cy + (height / 2) * Math.sin(a) * ripple,
    };
  });
}
/** Freeform shape paths share the document's native ink/undo/save pipeline. */
export function PathShapeLayer({
  documentId,
  pageIndex,
  pageWidth,
  pageHeight,
  enabled,
}: {
  documentId: string;
  pageIndex: number;
  pageWidth: number;
  pageHeight: number;
  enabled: boolean;
}) {
  const layer = useRef<HTMLDivElement>(null);
  const [kind, setKind] = useState<PathKind | null>(null);
  const [points, setPoints] = useState<InkPoint[]>([]);
  const [cursor, setCursor] = useState<InkPoint | null>(null);
  const [color, setColor] = useState("#e44234");
  const [lineWidth, setLineWidth] = useState(1);
  const [opacity, setOpacity] = useState(100);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const point = (event: PointerEvent): InkPoint => {
    const r = layer.current!.getBoundingClientRect();
    return {
      x: Math.max(
        0,
        Math.min(pageWidth, ((event.clientX - r.left) / r.width) * pageWidth),
      ),
      y: Math.max(
        0,
        Math.min(pageHeight, ((event.clientY - r.top) / r.height) * pageHeight),
      ),
    };
  };
  async function commit(stroke: InkPoint[]) {
    if (stroke.length < 2 || busy) return;
    setBusy(true);
    setError("");
    try {
      await createInkAnnotation(documentId, {
        pageIndex,
        color,
        opacity,
        lineWidth,
        strokes: [stroke],
      });
      setPoints([]);
      setCursor(null);
      notifyDocumentChanged({ documentId, pageIndex, source: "shape" });
    } catch (e) {
      setError(toCommandError(e).message);
    } finally {
      setBusy(false);
    }
  }
  const multipoint = kind === "polygon" || kind === "polyline";
  let preview: InkPoint[] = points;
  if (kind && cursor && points.length)
    preview = multipoint
      ? [...points, cursor, ...(kind === "polygon" ? [points[0]] : [])]
      : shapeStroke(kind, points[0], cursor);
  return (
    <div
      ref={layer}
      className={`path-shape-layer${kind ? " path-shape-layer--active" : ""}`}
      tabIndex={kind ? 0 : undefined}
      aria-label={kind ? `${names[kind]}绘制区域` : undefined}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          setPoints([]);
          setKind(null);
        } else if (e.key === "Enter" && multipoint) {
          e.preventDefault();
          void commit(kind === "polygon" ? [...points, points[0]] : points);
        }
      }}
      onPointerDown={(e) => {
        if (!kind || busy || e.button !== 0) return;
        e.preventDefault();
        e.stopPropagation();
        e.currentTarget.focus();
        const p = point(e);
        if (multipoint) {
          setPoints((ps) => [...ps, p]);
        } else {
          setPoints([p]);
          setCursor(p);
          e.currentTarget.setPointerCapture(e.pointerId);
        }
      }}
      onPointerMove={(e) => {
        if (kind && points.length) setCursor(point(e));
      }}
      onPointerUp={(e) => {
        if (kind && !multipoint && points.length) {
          void commit(shapeStroke(kind, points[0], point(e)));
        }
      }}
      onDoubleClick={(e) => {
        if (!multipoint) return;
        e.preventDefault();
        void commit(kind === "polygon" ? [...points, points[0]] : points);
      }}
    >
      <ToolShelf
        panelTitle={kind ? `${names[kind]}工具` : "形状"}
        tools={(Object.keys(names) as PathKind[]).map((k) => ({
          id: k,
          label: names[k],
          icon: icons[k],
          groups: ["shapes"],
          order: orders[k],
          active: kind === k,
          disabled: !enabled || busy,
          onActivate: (next) => {
            setKind(next ? k : null);
            setPoints([]);
            setCursor(null);
          },
        }))}
      >
        <div className="path-shape-properties">
          <label>
            边框颜色
            <input
              type="color"
              aria-label="边框颜色"
              value={color}
              onChange={(e) => setColor(e.target.value)}
            />
          </label>
          <label>
            厚度
            <input
              type="range"
              aria-label="厚度"
              min="0.5"
              max="12"
              step="0.5"
              value={lineWidth}
              onChange={(e) => setLineWidth(Number(e.target.value))}
            />
            <span>{lineWidth} pt</span>
          </label>
          <label>
            不透明度
            <input
              type="range"
              aria-label="不透明度"
              min="0"
              max="100"
              value={opacity}
              onChange={(e) => setOpacity(Number(e.target.value))}
            />
          </label>
          {multipoint && <p>单击添加顶点，双击或按 Enter 完成。</p>}
          {error && <p role="alert">{error}</p>}
        </div>
      </ToolShelf>
      {preview.length > 1 && (
        <svg
          className="path-shape-preview"
          viewBox={`0 0 ${pageWidth} ${pageHeight}`}
          preserveAspectRatio="none"
        >
          <polyline
            points={preview.map((p) => `${p.x},${p.y}`).join(" ")}
            fill="none"
            stroke={color}
            strokeWidth={lineWidth}
            strokeOpacity={opacity / 100}
            strokeLinejoin="round"
            strokeLinecap="round"
          />
        </svg>
      )}
    </div>
  );
}
