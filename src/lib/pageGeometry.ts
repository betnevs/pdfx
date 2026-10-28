/**
 * Geometry for the page viewer.
 *
 * All display coordinates in this module are in "display points": CSS pixels
 * at 100% zoom, measured from the top-left corner of the page as PDFium
 * renders it (intrinsic page rotation already applied). The page surface in
 * the DOM uses display pixels at the current scale, so a point on the surface
 * converts to display points by dividing by the scale, then to PDF user space
 * through `displayPointToPdfPoint`.
 */

/** Zoom is relative to 72 DPI; 1 is actual size. */
export const DEFAULT_SCALE = 1;
export const MIN_SCALE = 0.1;
export const MAX_SCALE = 40;
export const ZOOM_STEP = 1.25;

/**
 * Above this many pixels a full-page bitmap is no longer rendered in one
 * piece; the viewer asks the backend for the visible region instead.
 */
export const FULL_PAGE_RENDER_PIXEL_LIMIT = 12_000_000;

/** Long edge, in pixels, of a rendered thumbnail. */
export const THUMBNAIL_MAX_EDGE = 144;

export type Rotation = 0 | 90 | 180 | 270;
export type FitMode = "width" | "page";

export interface Point {
  x: number;
  y: number;
}

export interface PageSize {
  width: number;
  height: number;
}

/** Display metadata for a page, as returned by the backend. */
export interface PageMetadata extends PageSize {
  /** Intrinsic page rotation in degrees, clockwise. */
  rotation: number;
  /** Lower-left corner of the CropBox in PDF user space. */
  cropX: number;
  cropY: number;
}

export interface ViewportSize {
  width: number;
  height: number;
}

export interface ScrollViewport extends ViewportSize {
  scrollLeft: number;
  scrollTop: number;
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export function clampScale(scale: number): number {
  if (!Number.isFinite(scale)) {
    return DEFAULT_SCALE;
  }
  const bounded = Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
  return Math.round(bounded * 10_000) / 10_000;
}

/** Converts a zoom percentage from user input into a clamped scale. */
export function percentToScale(percent: number): number {
  if (!Number.isFinite(percent)) {
    return DEFAULT_SCALE;
  }
  return clampScale(percent / 100);
}

/** Rounds a scale to the whole percentage shown in the status bar. */
export function scaleToPercent(scale: number): number {
  return Math.round(clampScale(scale) * 100);
}

/**
 * Scale at which the page fits the viewport, clamped to the zoom limits.
 * Returns actual size until the viewport has been measured.
 */
export function fitScale(
  mode: FitMode,
  viewport: ViewportSize,
  page: PageSize,
): number {
  if (
    !(viewport.width > 0) ||
    !(viewport.height > 0) ||
    !(page.width > 0) ||
    !(page.height > 0)
  ) {
    return DEFAULT_SCALE;
  }

  const widthScale = viewport.width / page.width;
  const candidate =
    mode === "width"
      ? widthScale
      : Math.min(widthScale, viewport.height / page.height);
  return clampScale(candidate);
}

/** Whole-pixel size of a rendered page at a scale, matching the backend. */
export function pagePixelSize(page: PageSize, scale: number): PageSize {
  if (!Number.isFinite(scale) || scale <= 0) {
    return { width: 1, height: 1 };
  }
  return {
    width: Math.max(1, Math.round(page.width * scale)),
    height: Math.max(1, Math.round(page.height * scale)),
  };
}

/** Whether the page must be rendered as viewport-sized regions at this scale. */
export function shouldTile(page: PageSize, scale: number): boolean {
  const pixels = pagePixelSize(page, scale);
  return pixels.width * pixels.height > FULL_PAGE_RENDER_PIXEL_LIMIT;
}

/**
 * Scroll offset that keeps the page point under the viewport centre fixed
 * when the scale changes. `scrollOffset` is relative to the page origin.
 */
export function anchoredScroll(
  scrollOffset: number,
  viewportSize: number,
  previousScale: number,
  nextScale: number,
): number {
  if (!(previousScale > 0) || !(nextScale > 0)) {
    return Math.max(0, scrollOffset);
  }
  const ratio = nextScale / previousScale;
  const centred = (scrollOffset + viewportSize / 2) * ratio - viewportSize / 2;
  return Math.max(0, centred);
}

/**
 * The part of the page that intersects the viewport plus `overscan` pixels on
 * every side, in page display pixels. The result never exceeds the page.
 */
export function visiblePageRegion(
  viewport: ScrollViewport,
  page: PageSize,
  overscan: number,
): Rect {
  if (!(page.width > 0) || !(page.height > 0)) {
    return { x: 0, y: 0, width: 0, height: 0 };
  }

  const x = Math.min(page.width, Math.max(0, Math.floor(viewport.scrollLeft - overscan)));
  const y = Math.min(page.height, Math.max(0, Math.floor(viewport.scrollTop - overscan)));
  const width = Math.max(
    0,
    Math.min(page.width - x, Math.ceil(viewport.width + overscan * 2)),
  );
  const height = Math.max(
    0,
    Math.min(page.height - y, Math.ceil(viewport.height + overscan * 2)),
  );
  return { x, y, width, height };
}

export function normalizeRotation(rotation: number): Rotation {
  const normalized = ((Math.round(rotation / 90) * 90) % 360 + 360) % 360;
  return normalized as Rotation;
}

/**
 * Size of the page before intrinsic rotation. A 90/270 degree rotation swaps
 * the displayed width and height.
 */
export function unrotatedPageSize(page: PageMetadata): PageSize {
  const rotation = normalizeRotation(page.rotation);
  return rotation === 90 || rotation === 270
    ? { width: page.height, height: page.width }
    : { width: page.width, height: page.height };
}

/** Maps a display point (in display pixels at `scale`) to PDF user space. */
export function displayPointToPdfPoint(
  point: Point,
  page: PageMetadata,
  scale: number,
): Point {
  const safeScale = Number.isFinite(scale) && scale > 0 ? scale : 1;
  const dx = point.x / safeScale;
  const dy = point.y / safeScale;
  const { width, height } = unrotatedPageSize(page);

  let x: number;
  let y: number;
  switch (normalizeRotation(page.rotation)) {
    case 90:
      x = dy;
      y = dx;
      break;
    case 180:
      x = width - dx;
      y = height - dy;
      break;
    case 270:
      x = width - dy;
      y = height - dx;
      break;
    default:
      x = dx;
      y = height - dy;
      break;
  }

  return { x: x + page.cropX, y: y + page.cropY };
}

/** Inverse of `displayPointToPdfPoint`: PDF user space to display pixels. */
export function pdfPointToDisplayPoint(
  point: Point,
  page: PageMetadata,
  scale: number,
): Point {
  const safeScale = Number.isFinite(scale) && scale > 0 ? scale : 1;
  const px = point.x - page.cropX;
  const py = point.y - page.cropY;
  const { width, height } = unrotatedPageSize(page);

  let dx: number;
  let dy: number;
  switch (normalizeRotation(page.rotation)) {
    case 90:
      dx = py;
      dy = px;
      break;
    case 180:
      dx = width - px;
      dy = height - py;
      break;
    case 270:
      dx = height - py;
      dy = width - px;
      break;
    default:
      dx = px;
      dy = height - py;
      break;
  }

  return { x: dx * safeScale, y: dy * safeScale };
}
