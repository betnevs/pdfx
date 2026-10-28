/**
 * Display-space geometry for straight lines and arrows.
 *
 * A line is stored as its two endpoints in display-space points (top-left
 * origin, CropBox and intrinsic page rotation already applied), matching the
 * overlay and the backend's `LineGeometry`. There is no rectangle or stored
 * rotation: moving translates both endpoints and rotating turns them around
 * their midpoint, so the saved page path follows the overlay exactly.
 *
 * The arrow-head formula mirrors the backend's `arrow_head` function so the
 * live preview has the same head length and width as the saved content stream.
 */

/** Smallest drawable line, in display-space points. */
export const MIN_LINE_LENGTH = 2;

/** Arrow-head length is proportional to the stroke width, within these bounds. */
export const ARROW_HEAD_MIN_LENGTH = 8;
export const ARROW_HEAD_MAX_LENGTH = 24;
export const ARROW_HEAD_LENGTH_PER_WIDTH = 4;
/** Half-width of the arrow head as a fraction of its length. */
export const ARROW_HEAD_HALF_WIDTH_RATIO = 0.35;

export interface Point {
  x: number;
  y: number;
}

/** Two display-space endpoints; an arrow head sits at `(x2, y2)`. */
export interface LineGeometry {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
}

/** Which endpoint an endpoint drag moves. */
export type LineEndpoint = "start" | "end";

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(Math.max(value, minimum), maximum);
}

function clampPoint(point: Point, pageWidth: number, pageHeight: number): Point {
  return {
    x: clamp(point.x, 0, pageWidth),
    y: clamp(point.y, 0, pageHeight),
  };
}

/** Euclidean length of the segment, in display-space points. */
export function lineLength(geometry: LineGeometry): number {
  const dx = geometry.x2 - geometry.x1;
  const dy = geometry.y2 - geometry.y1;
  return Math.sqrt(dx * dx + dy * dy);
}

/** Midpoint of the segment, used as the rotation centre. */
export function lineMidpoint(geometry: LineGeometry): Point {
  return {
    x: (geometry.x1 + geometry.x2) / 2,
    y: (geometry.y1 + geometry.y2) / 2,
  };
}

/**
 * Angle of the segment in display-space degrees: 0 points right, 90 points
 * down (the page's y axis grows downwards). Normalised to 0-360.
 */
export function lineAngle(geometry: LineGeometry): number {
  const degrees =
    (Math.atan2(geometry.y2 - geometry.y1, geometry.x2 - geometry.x1) * 180) /
    Math.PI;
  return ((degrees % 360) + 360) % 360;
}

/**
 * The page-clamped segment a create drag describes.
 *
 * Both endpoints are clamped to the page and a click or a drag shorter than
 * [`MIN_LINE_LENGTH`] returns null, so the caller leaves the document
 * untouched instead of creating an invisible object.
 */
export function lineFromDrag(
  anchor: Point,
  current: Point,
  pageWidth: number,
  pageHeight: number,
): LineGeometry | null {
  const start = clampPoint(anchor, pageWidth, pageHeight);
  const end = clampPoint(current, pageWidth, pageHeight);
  const geometry = {
    x1: start.x,
    y1: start.y,
    x2: end.x,
    y2: end.y,
  };
  return lineLength(geometry) < MIN_LINE_LENGTH ? null : geometry;
}

/**
 * Translates both endpoints by a drag delta, keeping the whole segment inside
 * the page. The translation is clamped as a whole, so moving never changes the
 * line's length or angle.
 */
export function moveLine(
  geometry: LineGeometry,
  delta: Point,
  pageWidth: number,
  pageHeight: number,
): LineGeometry {
  const minX = Math.min(geometry.x1, geometry.x2);
  const maxX = Math.max(geometry.x1, geometry.x2);
  const minY = Math.min(geometry.y1, geometry.y2);
  const maxY = Math.max(geometry.y1, geometry.y2);
  // A segment at the page edge can leave no room to move further; clamp keeps
  // the lower bound first so the range never inverts.
  const dx = clamp(delta.x, -minX, Math.max(pageWidth - maxX, -minX));
  const dy = clamp(delta.y, -minY, Math.max(pageHeight - maxY, -minY));
  return {
    x1: clamp(geometry.x1 + dx, 0, pageWidth),
    y1: clamp(geometry.y1 + dy, 0, pageHeight),
    x2: clamp(geometry.x2 + dx, 0, pageWidth),
    y2: clamp(geometry.y2 + dy, 0, pageHeight),
  };
}

/**
 * The segment produced by dragging one endpoint to `point`.
 *
 * Only the dragged endpoint changes; it is clamped to the page. A drag that
 * would make the segment too short is still returned here and rejected by the
 * caller, so the handle keeps following the pointer while the previous
 * committed geometry stays intact.
 */
export function moveLineEndpoint(
  geometry: LineGeometry,
  endpoint: LineEndpoint,
  point: Point,
  pageWidth: number,
  pageHeight: number,
): LineGeometry {
  const clamped = clampPoint(point, pageWidth, pageHeight);
  return endpoint === "start"
    ? { ...geometry, x1: clamped.x, y1: clamped.y }
    : { ...geometry, x2: clamped.x, y2: clamped.y };
}

/**
 * Rotates both endpoints clockwise around the midpoint by `degrees`,
 * preserving the segment's length.
 */
export function rotateLine(geometry: LineGeometry, degrees: number): LineGeometry {
  const centre = lineMidpoint(geometry);
  const radians = (degrees * Math.PI) / 180;
  const cos = Math.cos(radians);
  const sin = Math.sin(radians);
  const rotate = (point: Point): Point => {
    const dx = point.x - centre.x;
    const dy = point.y - centre.y;
    return {
      x: centre.x + dx * cos - dy * sin,
      y: centre.y + dx * sin + dy * cos,
    };
  };
  const start = rotate({ x: geometry.x1, y: geometry.y1 });
  const end = rotate({ x: geometry.x2, y: geometry.y2 });
  return { x1: start.x, y1: start.y, x2: end.x, y2: end.y };
}

/**
 * Rotation delta for a rotate-handle drag, in degrees.
 *
 * The pointer angle around the segment's midpoint is compared with its angle
 * when the drag started, so grabbing the handle never makes the line jump.
 * `snapDegrees` (for example 15 while Shift is held) rounds the resulting
 * absolute line angle.
 */
export function lineRotationFromPointer(
  geometry: LineGeometry,
  startPointer: Point,
  currentPointer: Point,
  snapDegrees = 0,
): number {
  const centre = lineMidpoint(geometry);
  const angleAt = (point: Point): number =>
    (Math.atan2(point.y - centre.y, point.x - centre.x) * 180) / Math.PI;
  const delta = angleAt(currentPointer) - angleAt(startPointer);
  if (snapDegrees <= 0) {
    return delta;
  }
  const startAngle = lineAngle(geometry);
  const snapped = Math.round((startAngle + delta) / snapDegrees) * snapDegrees;
  return snapped - startAngle;
}

/**
 * The three display-space points of the arrow head: tip, left base, right
 * base. Mirrors the backend's serialized head exactly (including the clamp on
 * short lines) so preview and saved output agree.
 */
export function arrowHeadPoints(
  geometry: LineGeometry,
  lineWidth: number,
): [Point, Point, Point] {
  const dx = geometry.x2 - geometry.x1;
  const dy = geometry.y2 - geometry.y1;
  const length = Math.max(
    Math.sqrt(dx * dx + dy * dy),
    Number.MIN_VALUE,
  );
  const ux = dx / length;
  const uy = dy / length;
  const headLength = Math.min(
    clamp(
      lineWidth * ARROW_HEAD_LENGTH_PER_WIDTH,
      ARROW_HEAD_MIN_LENGTH,
      ARROW_HEAD_MAX_LENGTH,
    ),
    length,
  );
  const halfWidth = headLength * ARROW_HEAD_HALF_WIDTH_RATIO;
  const baseX = geometry.x2 - ux * headLength;
  const baseY = geometry.y2 - uy * headLength;
  return [
    { x: geometry.x2, y: geometry.y2 },
    { x: baseX - uy * halfWidth, y: baseY + ux * halfWidth },
    { x: baseX + uy * halfWidth, y: baseY - ux * halfWidth },
  ];
}

/** SVG `points` attribute for the arrow head triangle. */
export function arrowHeadPointsAttribute(
  geometry: LineGeometry,
  lineWidth: number,
): string {
  return arrowHeadPoints(geometry, lineWidth)
    .map((point) => `${point.x},${point.y}`)
    .join(" ");
}

/** `rotate(angle cx cy)` transform for an SVG element around the midpoint. */
export function lineRotationTransform(geometry: LineGeometry, degrees: number): string {
  const centre = lineMidpoint(geometry);
  return `rotate(${degrees} ${centre.x} ${centre.y})`;
}
