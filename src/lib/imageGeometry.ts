/** Smallest image edge the overlay lets the user drag to, in page points. */
export const MIN_IMAGE_SIZE = 1;

/** A rectangle in display-space page points (top-left origin). */
export interface ImageRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Point {
  x: number;
  y: number;
}

/** The rectangle's center, which is the pivot for rotation and resizing. */
export function imageCenter(rect: ImageRect): Point {
  return {
    x: rect.x + rect.width / 2,
    y: rect.y + rect.height / 2,
  };
}

/**
 * Translates an image by the drag delta. The image may extend past the page;
 * the page clips it when rendering and saving, and the overlay warns.
 */
export function moveImage(rect: ImageRect, delta: Point): ImageRect {
  return {
    ...rect,
    x: rect.x + delta.x,
    y: rect.y + delta.y,
  };
}

/**
 * The rectangle produced by dragging the bottom-right handle to `pointer`.
 *
 * The pointer is converted into the image's unrotated local space around the
 * center, so the drag follows the image's own axes when it is rotated. The
 * aspect ratio and center are preserved, and both edges keep a minimum size.
 */
export function resizeImage(
  rect: ImageRect,
  rotationDegrees: number,
  pointer: Point,
): ImageRect {
  const center = imageCenter(rect);
  const radians = (rotationDegrees * Math.PI) / 180;
  const cos = Math.cos(radians);
  const sin = Math.sin(radians);
  const dx = pointer.x - center.x;
  const dy = pointer.y - center.y;
  // Inverse of the clockwise rotation CSS applies around the center.
  const localX = dx * cos + dy * sin;
  const localY = -dx * sin + dy * cos;
  const halfWidth = rect.width / 2;
  const halfHeight = rect.height / 2;

  const projected = (localX * halfWidth + localY * halfHeight) /
    (halfWidth * halfWidth + halfHeight * halfHeight);
  const minScale = Math.max(
    MIN_IMAGE_SIZE / rect.width,
    MIN_IMAGE_SIZE / rect.height,
  );
  const scale = Math.max(Number.isFinite(projected) ? projected : 1, minScale);
  const width = rect.width * scale;
  const height = rect.height * scale;
  return {
    x: center.x - width / 2,
    y: center.y - height / 2,
    width,
    height,
  };
}

/**
 * Clockwise rotation for a rotate-handle drag, in degrees, normalized to
 * `(-180, 180]` like the backend. At rotation 0 the handle sits above the
 * center, so a pointer straight above the center means 0 degrees.
 */
export function imageRotationFromPointer(
  rect: ImageRect,
  pointer: Point,
  snapDegrees = 0,
): number {
  const center = imageCenter(rect);
  const angle =
    (Math.atan2(pointer.y - center.y, pointer.x - center.x) * 180) / Math.PI +
    90;
  const snapped =
    snapDegrees > 0 ? Math.round(angle / snapDegrees) * snapDegrees : angle;
  const normalized = ((((snapped + 180) % 360) + 360) % 360) - 180;
  return normalized === -180 ? 180 : normalized;
}

/** The rectangle's four corners after clockwise rotation around its center. */
export function imageCorners(
  rect: ImageRect,
  rotationDegrees: number,
): [Point, Point, Point, Point] {
  const center = imageCenter(rect);
  const radians = (rotationDegrees * Math.PI) / 180;
  const cos = Math.cos(radians);
  const sin = Math.sin(radians);
  const halfWidth = rect.width / 2;
  const halfHeight = rect.height / 2;
  const local: Array<[number, number]> = [
    [-halfWidth, -halfHeight],
    [halfWidth, -halfHeight],
    [halfWidth, halfHeight],
    [-halfWidth, halfHeight],
  ];
  return local.map(([dx, dy]) => ({
    x: center.x + dx * cos - dy * sin,
    y: center.y + dx * sin + dy * cos,
  })) as [Point, Point, Point, Point];
}

/** Tolerance for the page-edge check; PDF boxes are floating point. */
const EDGE_EPSILON = 0.01;

/**
 * True when any rotated corner falls outside the page, so part of the image
 * is clipped. Clipping is the documented policy: the page (and the saved PDF)
 * shows only the part inside the page box.
 */
export function isImageOutsidePage(
  rect: ImageRect,
  rotationDegrees: number,
  pageWidth: number,
  pageHeight: number,
): boolean {
  return imageCorners(rect, rotationDegrees).some(
    (corner) =>
      corner.x < -EDGE_EPSILON ||
      corner.y < -EDGE_EPSILON ||
      corner.x > pageWidth + EDGE_EPSILON ||
      corner.y > pageHeight + EDGE_EPSILON,
  );
}
