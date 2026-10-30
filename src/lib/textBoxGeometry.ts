/** Minimum size of a Text Box, in display-space points. */
export const MIN_BOX_WIDTH = 40;
export const MIN_BOX_HEIGHT = 20;

/** Size used when the user clicks instead of dragging to create a Text Box. */
export const DEFAULT_BOX_WIDTH = 200;
export const DEFAULT_BOX_HEIGHT = 32;

/** A rectangle in display-space page points (top-left origin). */
export interface BoxRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Point {
  x: number;
  y: number;
}

/** Drags shorter than this on either axis count as a click, not a box. */
const CLICK_SLOP = 12;

/**
 * The box a create drag describes. A click (or a drag too small to be
 * intentional) creates a default-sized box at the anchor.
 */
export function dragRect(anchor: Point, current: Point): BoxRect {
  const dx = current.x - anchor.x;
  const dy = current.y - anchor.y;
  if (Math.abs(dx) < CLICK_SLOP || Math.abs(dy) < CLICK_SLOP) {
    return {
      x: anchor.x,
      y: anchor.y,
      width: DEFAULT_BOX_WIDTH,
      height: DEFAULT_BOX_HEIGHT,
    };
  }
  return {
    x: Math.min(anchor.x, current.x),
    y: Math.min(anchor.y, current.y),
    width: Math.abs(dx),
    height: Math.abs(dy),
  };
}

/**
 * Keeps a box inside the page. Over-wide or over-tall boxes keep their
 * top-left corner and are reduced to the remaining page area.
 */
export function fitBoxToPage(
  rect: BoxRect,
  pageWidth: number,
  pageHeight: number,
): BoxRect {
  const x = Math.min(
    Math.max(rect.x, 0),
    Math.max(pageWidth - MIN_BOX_WIDTH, 0),
  );
  const y = Math.min(
    Math.max(rect.y, 0),
    Math.max(pageHeight - MIN_BOX_HEIGHT, 0),
  );
  return {
    x,
    y,
    width: Math.max(Math.min(rect.width, pageWidth - x), MIN_BOX_WIDTH),
    height: Math.max(Math.min(rect.height, pageHeight - y), MIN_BOX_HEIGHT),
  };
}

/**
 * Translates a box by the drag delta and keeps it inside the page. The size
 * is preserved, so moving a box never changes its wrapping or height.
 */
export function moveBox(
  rect: BoxRect,
  delta: Point,
  pageWidth: number,
  pageHeight: number,
): BoxRect {
  const maxX = Math.max(pageWidth - rect.width, 0);
  const maxY = Math.max(pageHeight - rect.height, 0);
  return {
    ...rect,
    x: Math.min(Math.max(rect.x + delta.x, 0), maxX),
    y: Math.min(Math.max(rect.y + delta.y, 0), maxY),
  };
}

/**
 * The box produced by dragging the bottom-right handle to `pointer`.
 *
 * The pointer is converted into the box's unrotated local space, so resizing
 * a rotated box follows its own axes. The top-left corner and rotation are
 * unchanged; only width and height grow or shrink, with a minimum size.
 */
export function resizeBox(
  rect: BoxRect,
  rotationDegrees: number,
  pointer: Point,
): BoxRect {
  const radians = (rotationDegrees * Math.PI) / 180;
  const dx = pointer.x - rect.x;
  const dy = pointer.y - rect.y;
  const cos = Math.cos(radians);
  const sin = Math.sin(radians);
  const localX = dx * cos + dy * sin;
  const localY = -dx * sin + dy * cos;
  return {
    ...rect,
    width: Math.max(localX, MIN_BOX_WIDTH),
    height: Math.max(localY, MIN_BOX_HEIGHT),
  };
}

/**
 * Clockwise rotation for a rotate-handle drag, in degrees.
 *
 * The angle is measured from the box's top-left corner around to the
 * pointer, matching CSS `rotate()` and the backend's display-space rotation.
 * `snapDegrees` (for example 15 while Shift is held) rounds to that step.
 */
export function rotationFromPointer(
  rect: BoxRect,
  pointer: Point,
  snapDegrees = 0,
): number {
  const angle =
    (Math.atan2(pointer.y - rect.y, pointer.x - rect.x) * 180) / Math.PI;
  const snapped =
    snapDegrees > 0 ? Math.round(angle / snapDegrees) * snapDegrees : angle;
  return ((snapped % 360) + 360) % 360;
}

/** Line height as a multiple of font size, matching the backend layout. */
export const LINE_HEIGHT_FACTOR = 1.2;

const AVERAGE_GLYPH_WIDTH: Record<string, number> = {
  Helvetica: 0.52,
  Times: 0.48,
  Courier: 0.6,
};

function glyphWidth(
  character: string,
  fontSize: number,
  fontFamily: string,
): number {
  if (character === " ") {
    return fontSize * 0.28;
  }
  if (fontFamily === "Courier" || fontFamily.includes("Courier")) {
    return fontSize * 0.6;
  }
  if (fontFamily === "Times" || fontFamily.includes("Times")) {
    return fontSize * 0.48;
  }
  return fontSize * (AVERAGE_GLYPH_WIDTH[fontFamily] ?? 0.52);
}

/**
 * Approximate number of lines `text` needs when wrapped at `width`.
 *
 * The backend measures real font metrics when it lays the box out; this
 * estimate exists so the inline editor can grow while the user types. It
 * breaks on explicit newlines, wraps at spaces and splits over-long words.
 */
export function wrappedLineCount(
  text: string,
  width: number,
  fontSize: number,
  fontFamily: string,
): number {
  const wrapWidth = Math.max(width, 1);
  let lines = 0;
  for (const paragraph of text.split("\n")) {
    if (paragraph.length === 0) {
      lines += 1;
      continue;
    }
    const words = paragraph.split(" ");
    let currentWidth = 0;
    let paragraphLines = 1;
    for (let index = 0; index < words.length; index += 1) {
      const word = words[index];
      const wordWidth = [...word].reduce(
        (total, character) =>
          total + glyphWidth(character, fontSize, fontFamily),
        0,
      );
      const spaceWidth = index === 0 ? 0 : fontSize * 0.28;
      if (
        currentWidth > 0 &&
        currentWidth + spaceWidth + wordWidth > wrapWidth
      ) {
        paragraphLines += 1;
        currentWidth = wordWidth;
      } else {
        currentWidth += spaceWidth + wordWidth;
      }
      if (currentWidth > wrapWidth) {
        const extraLines = Math.ceil(currentWidth / wrapWidth) - 1;
        paragraphLines += extraLines;
        currentWidth -= extraLines * wrapWidth;
      }
    }
    lines += paragraphLines;
  }
  return Math.max(lines, 1);
}

/** Estimated content height for the inline editor's live preview. */
export function estimatedContentHeight(
  text: string,
  width: number,
  fontSize: number,
  fontFamily: string,
): number {
  return (
    wrappedLineCount(text, width, fontSize, fontFamily) *
    fontSize *
    LINE_HEIGHT_FACTOR
  );
}

export type ResizeHandle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w";
export const RESIZE_HANDLES: readonly ResizeHandle[] = [
  "nw",
  "n",
  "ne",
  "e",
  "se",
  "s",
  "sw",
  "w",
];
/** Resize in the box's axes, keeping the opposite edge/corner fixed. Font size is never scaled. */
export function resizeBoxFromHandle(
  rect: BoxRect,
  rotationDegrees: number,
  pointer: Point,
  handle: ResizeHandle,
): BoxRect {
  const radians = (rotationDegrees * Math.PI) / 180;
  const cos = Math.cos(radians),
    sin = Math.sin(radians);
  const dx = pointer.x - rect.x,
    dy = pointer.y - rect.y;
  const localX = dx * cos + dy * sin,
    localY = -dx * sin + dy * cos;
  let left = 0,
    top = 0,
    right = rect.width,
    bottom = rect.height;
  if (handle.includes("w")) left = Math.min(localX, right - 4);
  if (handle.includes("e")) right = Math.max(localX, 4);
  if (handle.includes("n")) top = Math.min(localY, bottom - 4);
  if (handle.includes("s")) bottom = Math.max(localY, 4);
  return {
    x: rect.x + left * cos - top * sin,
    y: rect.y + left * sin + top * cos,
    width: right - left,
    height: bottom - top,
  };
}

/** A screen-space selection frame gives every handle room at every zoom. */
export function contentSelectionFrame(rect: BoxRect, rotation: number, scale: number): BoxRect {
  const zoom = Math.max(scale, 0.001);
  const px = Math.max(6, (28 - rect.width * zoom) / 2) / zoom;
  const py = Math.max(6, (28 - rect.height * zoom) / 2) / zoom;
  const radians = rotation * Math.PI / 180;
  return {
    x: rect.x - px * Math.cos(radians) + py * Math.sin(radians),
    y: rect.y - px * Math.sin(radians) - py * Math.cos(radians),
    width: rect.width + 2 * px,
    height: rect.height + 2 * py,
  };
}

/** Use pointer deltas so padding and the point grabbed inside a handle cannot cause a jump. */
export function resizeBoxFromDrag(
  rect: BoxRect, rotation: number, start: Point, current: Point, handle: ResizeHandle,
): BoxRect {
  const x = handle.includes("w") ? 0 : handle.includes("e") ? rect.width : rect.width / 2;
  const y = handle.includes("n") ? 0 : handle.includes("s") ? rect.height : rect.height / 2;
  const radians = rotation * Math.PI / 180;
  return resizeBoxFromHandle(rect, rotation, {
    x: rect.x + x * Math.cos(radians) - y * Math.sin(radians) + current.x - start.x,
    y: rect.y + x * Math.sin(radians) + y * Math.cos(radians) + current.y - start.y,
  }, handle);
}
