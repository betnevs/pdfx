/**
 * Display-space geometry for rectangle and ellipse annotations.
 *
 * Shape geometry is stored unrotated with a clockwise rotation around its
 * centre. These helpers keep the overlay, the drag preview and the values sent
 * to the backend in the same coordinate system (top-left origin page points).
 */

/** Smallest drawable shape edge, in display-space points. */
export const MIN_SHAPE_SIZE = 2;

/** A rectangle in display-space page points. */
export interface ShapeRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** The geometry fields these helpers transform; matches the backend view. */
export interface ShapeGeometryLike extends ShapeRect {
  /** Clockwise degrees around the rectangle's centre. */
  rotation: number;
}

export interface Point {
  x: number;
  y: number;
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(Math.max(value, minimum), maximum);
}

function radians(degrees: number): number {
  return (degrees * Math.PI) / 180;
}

function normalizeDegrees(degrees: number): number {
  return ((degrees % 360) + 360) % 360;
}

function clampPoint(point: Point, pageWidth: number, pageHeight: number): Point {
  return {
    x: clamp(point.x, 0, pageWidth),
    y: clamp(point.y, 0, pageHeight),
  };
}

/**
 * The page-clamped rectangle a create drag describes.
 *
 * Returns null for a click or a drag too small to be intentional, so the
 * caller can leave the document untouched instead of creating a sliver.
 */
export function shapeRectFromDrag(
  anchor: Point,
  current: Point,
  pageWidth: number,
  pageHeight: number,
): ShapeRect | null {
  const start = clampPoint(anchor, pageWidth, pageHeight);
  const end = clampPoint(current, pageWidth, pageHeight);
  const x = Math.min(start.x, end.x);
  const y = Math.min(start.y, end.y);
  const width = Math.abs(end.x - start.x);
  const height = Math.abs(end.y - start.y);
  if (width < MIN_SHAPE_SIZE || height < MIN_SHAPE_SIZE) {
    return null;
  }
  return { x, y, width, height };
}

/**
 * The axis-aligned bounds of a rotated shape, in display space.
 *
 * Kept in step with the backend's padded `/Rect` calculation: the corners of
 * the local rectangle are rotated around the centre and their extents taken.
 */
export function shapeBounds(geometry: ShapeGeometryLike): ShapeRect {
  const angle = radians(geometry.rotation);
  const cos = Math.abs(Math.cos(angle));
  const sin = Math.abs(Math.sin(angle));
  const width = geometry.width * cos + geometry.height * sin;
  const height = geometry.width * sin + geometry.height * cos;
  const centreX = geometry.x + geometry.width / 2;
  const centreY = geometry.y + geometry.height / 2;
  return {
    x: centreX - width / 2,
    y: centreY - height / 2,
    width,
    height,
  };
}

/**
 * Translates a shape by a drag delta and keeps its rotated bounds inside the
 * page. Size and rotation are preserved, so moving never changes the shape.
 */
export function moveShape(
  geometry: ShapeGeometryLike,
  delta: Point,
  pageWidth: number,
  pageHeight: number,
): ShapeGeometryLike {
  const bounds = shapeBounds(geometry);
  const centreX = geometry.x + geometry.width / 2 + delta.x;
  const centreY = geometry.y + geometry.height / 2 + delta.y;
  const minimumX = bounds.width / 2;
  const minimumY = bounds.height / 2;
  const maximumX = Math.max(pageWidth - bounds.width / 2, minimumX);
  const maximumY = Math.max(pageHeight - bounds.height / 2, minimumY);
  const clampedX = clamp(centreX, minimumX, maximumX);
  const clampedY = clamp(centreY, minimumY, maximumY);
  return {
    ...geometry,
    x: clampedX - geometry.width / 2,
    y: clampedY - geometry.height / 2,
  };
}

/**
 * The shape produced by dragging the bottom-right handle to `pointer`.
 *
 * The top-left corner and rotation stay fixed. The pointer is projected onto
 * the shape's rotated local axes, so a rotated shape resizes along its own
 * width and height instead of the screen axes. Dragging across the fixed
 * corner clamps to the minimum size rather than flipping the rectangle.
 */
export function resizeShape(
  geometry: ShapeGeometryLike,
  pointer: Point,
  pageWidth: number,
  pageHeight: number,
): ShapeGeometryLike {
  const angle = radians(geometry.rotation);
  const cos = Math.cos(angle);
  const sin = Math.sin(angle);
  const centreX = geometry.x + geometry.width / 2;
  const centreY = geometry.y + geometry.height / 2;
  // Fixed opposite corner in display space.
  const fixedX = centreX - (geometry.width / 2) * cos + (geometry.height / 2) * sin;
  const fixedY = centreY - (geometry.width / 2) * sin - (geometry.height / 2) * cos;
  const target = clampPoint(pointer, pageWidth, pageHeight);
  const dx = target.x - fixedX;
  const dy = target.y - fixedY;
  const localX = dx * cos + dy * sin;
  const localY = -dx * sin + dy * cos;
  const width = Math.max(localX, MIN_SHAPE_SIZE);
  const height = Math.max(localY, MIN_SHAPE_SIZE);
  const newCentreX = fixedX + (width / 2) * cos - (height / 2) * sin;
  const newCentreY = fixedY + (width / 2) * sin + (height / 2) * cos;
  return {
    ...geometry,
    x: newCentreX - width / 2,
    y: newCentreY - height / 2,
    width,
    height,
  };
}

/**
 * Clockwise rotation for a rotate-handle drag, in degrees.
 *
 * The delta between the pointer's starting and current angles around the
 * shape centre is added to the rotation the shape had when the drag started,
 * so grabbing the handle never makes the shape jump. `snapDegrees` (for
 * example 15 while Shift is held) rounds the result.
 */
export function shapeRotationFromPointer(
  centre: Point,
  start: Point,
  current: Point,
  startRotation: number,
  snapDegrees = 0,
): number {
  const startAngle =
    (Math.atan2(start.y - centre.y, start.x - centre.x) * 180) / Math.PI;
  const currentAngle =
    (Math.atan2(current.y - centre.y, current.x - centre.x) * 180) / Math.PI;
  const rotation = startRotation + (currentAngle - startAngle);
  const snapped =
    snapDegrees > 0
      ? Math.round(rotation / snapDegrees) * snapDegrees
      : rotation;
  return normalizeDegrees(snapped);
}
