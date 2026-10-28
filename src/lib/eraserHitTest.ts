import type {
  AnnotationView,
  InkPoint,
  ShapeGeometry,
  StampFrameView,
  TextMarkupRect,
} from "./backend";

/** A target found under the eraser pointer. */
export interface EraserHit {
  annotation: AnnotationView;
  /**
   * Ink stroke to remove. Null means the whole annotation is the target,
   * either because it is not ink or because `wholeInk` was requested.
   */
  strokeIndex: number | null;
}

export interface EraserHitOptions {
  /** Display-space distance in points within which an ink stroke is hit. */
  radius: number;
  /** Hit whole ink annotations instead of individual strokes (Shift-click). */
  wholeInk?: boolean;
  /** Ignore annotations that belong to other pages. */
  pageIndex?: number;
}

const DEFAULT_INK_WIDTH = 2;

function containsRect(rect: TextMarkupRect, point: InkPoint): boolean {
  return (
    point.x >= rect.x &&
    point.x <= rect.x + rect.width &&
    point.y >= rect.y &&
    point.y <= rect.y + rect.height
  );
}

/** Rotates a point by `degrees` clockwise around the origin. */
function rotate(point: InkPoint, degrees: number): InkPoint {
  const radians = (degrees * Math.PI) / 180;
  const cos = Math.cos(radians);
  const sin = Math.sin(radians);
  return {
    x: point.x * cos - point.y * sin,
    y: point.x * sin + point.y * cos,
  };
}

/** Maps a display point into a frame's unrotated local coordinates. */
function toLocal(point: InkPoint, frame: TextMarkupRect & { rotation: number }): InkPoint {
  const centreX = frame.x + frame.width / 2;
  const centreY = frame.y + frame.height / 2;
  return rotate(
    { x: point.x - centreX, y: point.y - centreY },
    -frame.rotation,
  );
}

function insideRectangle(
  point: InkPoint,
  frame: TextMarkupRect & { rotation: number },
): boolean {
  if (frame.width <= 0 || frame.height <= 0) {
    // Degenerate frames still have a hit area at their origin.
    return point.x === frame.x && point.y === frame.y;
  }
  const local = toLocal(point, frame);
  return (
    Math.abs(local.x) <= frame.width / 2 &&
    Math.abs(local.y) <= frame.height / 2
  );
}

function insideEllipse(
  point: InkPoint,
  frame: TextMarkupRect & { rotation: number },
): boolean {
  if (frame.width <= 0 || frame.height <= 0) {
    return false;
  }
  const local = toLocal(point, frame);
  const radiusX = frame.width / 2;
  const radiusY = frame.height / 2;
  return (
    (local.x * local.x) / (radiusX * radiusX) +
      (local.y * local.y) / (radiusY * radiusY) <=
    1
  );
}

function geometryFrame(
  geometry: ShapeGeometry,
): TextMarkupRect & { rotation: number } {
  return {
    x: geometry.x,
    y: geometry.y,
    width: geometry.width,
    height: geometry.height,
    rotation: geometry.rotation,
  };
}

function stampFrame(
  frame: StampFrameView,
): TextMarkupRect & { rotation: number } {
  return {
    x: frame.x,
    y: frame.y,
    width: frame.width,
    height: frame.height,
    rotation: frame.rotation,
  };
}

/** Whole-annotation hit test: shapes and stamps honour their rotation. */
function hitsAnnotationBounds(annotation: AnnotationView, point: InkPoint): boolean {
  if (annotation.shape) {
    const frame = geometryFrame(annotation.shape.geometry);
    return annotation.shape.kind === "circle"
      ? insideEllipse(point, frame)
      : insideRectangle(point, frame);
  }
  if (annotation.stamp) {
    return insideRectangle(point, stampFrame(annotation.stamp.frame));
  }
  return annotation.rects.some((rect) => containsRect(rect, point));
}

function distanceToSegment(point: InkPoint, start: InkPoint, end: InkPoint): number {
  const dx = end.x - start.x;
  const dy = end.y - start.y;
  const lengthSquared = dx * dx + dy * dy;
  if (lengthSquared === 0) {
    return Math.hypot(point.x - start.x, point.y - start.y);
  }
  const t = Math.min(
    1,
    Math.max(
      0,
      ((point.x - start.x) * dx + (point.y - start.y) * dy) / lengthSquared,
    ),
  );
  return Math.hypot(point.x - (start.x + t * dx), point.y - (start.y + t * dy));
}

function distanceToStroke(point: InkPoint, stroke: InkPoint[]): number {
  if (stroke.length === 0) {
    return Number.POSITIVE_INFINITY;
  }
  if (stroke.length === 1) {
    return Math.hypot(point.x - stroke[0].x, point.y - stroke[0].y);
  }
  let nearest = Number.POSITIVE_INFINITY;
  for (let index = 1; index < stroke.length; index += 1) {
    nearest = Math.min(
      nearest,
      distanceToSegment(point, stroke[index - 1], stroke[index]),
    );
  }
  return nearest;
}

function hitInkStroke(
  annotation: AnnotationView,
  point: InkPoint,
  radius: number,
): number | null {
  const threshold =
    Math.max(0, radius) + (annotation.lineWidth ?? DEFAULT_INK_WIDTH) / 2;
  let nearestIndex: number | null = null;
  let nearestDistance = Number.POSITIVE_INFINITY;
  annotation.strokes.forEach((stroke, index) => {
    const distance = distanceToStroke(point, stroke);
    // `<=` lets a visually topmost (later) annotation win exact ties.
    if (distance <= threshold && distance <= nearestDistance) {
      nearestIndex = index;
      nearestDistance = distance;
    }
  });
  return nearestIndex;
}

/**
 * Finds the annotation (or ink stroke) an eraser click should affect.
 *
 * Shapes, stamps, notes, markup and future text annotations are hit-tested
 * before ink because their overlays draw above the text layer. Within each
 * group the later annotation wins, matching PDF paint order. Ink only hits
 * near a stroke, never anywhere inside its bounding box, so erasing the
 * interior of a closed scribble is a miss.
 */
export function hitTestEraser(
  annotations: AnnotationView[],
  point: InkPoint,
  options: EraserHitOptions,
): EraserHit | null {
  const visible = annotations.filter(
    (annotation) =>
      options.pageIndex === undefined || annotation.pageIndex === options.pageIndex,
  );

  for (let index = visible.length - 1; index >= 0; index -= 1) {
    const annotation = visible[index];
    if (annotation.kind === "ink") {
      continue;
    }
    if (hitsAnnotationBounds(annotation, point)) {
      return { annotation, strokeIndex: null };
    }
  }

  let nearest: EraserHit | null = null;
  let nearestDistance = Number.POSITIVE_INFINITY;
  for (const annotation of visible) {
    if (annotation.kind !== "ink") {
      continue;
    }
    const strokeIndex = hitInkStroke(annotation, point, options.radius);
    if (strokeIndex === null) {
      continue;
    }
    const distance = distanceToStroke(point, annotation.strokes[strokeIndex]);
    if (distance <= nearestDistance) {
      nearestDistance = distance;
      nearest = {
        annotation,
        strokeIndex: options.wholeInk ? null : strokeIndex,
      };
    }
  }
  return nearest;
}
