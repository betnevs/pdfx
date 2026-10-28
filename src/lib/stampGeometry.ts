import type { StampFrameView } from "./backend";

/** Minimum stamp size, matching the backend's creation/transform validation. */
export const MIN_STAMP_WIDTH = 30;
export const MIN_STAMP_HEIGHT = 10;

/** Size used when the user clicks a page to place a stamp. */
export const DEFAULT_STAMP_WIDTH = 180;
export const DEFAULT_STAMP_HEIGHT = 60;

export interface Point {
  x: number;
  y: number;
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(Math.max(value, minimum), Math.max(maximum, minimum));
}

export function stampCenter(frame: StampFrameView): Point {
  return {
    x: frame.x + frame.width / 2,
    y: frame.y + frame.height / 2,
  };
}

/**
 * A default-sized stamp centred on a placement click and kept inside the
 * page. Stamps start unrotated.
 */
export function placeStamp(
  point: Point,
  pageWidth: number,
  pageHeight: number,
): StampFrameView {
  const width = Math.max(Math.min(DEFAULT_STAMP_WIDTH, pageWidth), MIN_STAMP_WIDTH);
  const height = Math.max(
    Math.min(DEFAULT_STAMP_HEIGHT, pageHeight),
    MIN_STAMP_HEIGHT,
  );
  return {
    x: clamp(point.x - width / 2, 0, pageWidth - width),
    y: clamp(point.y - height / 2, 0, pageHeight - height),
    width,
    height,
    rotation: 0,
  };
}

/** Translates a stamp and keeps its frame inside the page. */
export function moveStamp(
  frame: StampFrameView,
  delta: Point,
  pageWidth: number,
  pageHeight: number,
): StampFrameView {
  return {
    ...frame,
    x: clamp(frame.x + delta.x, 0, pageWidth - frame.width),
    y: clamp(frame.y + delta.y, 0, pageHeight - frame.height),
  };
}

/**
 * The frame produced by dragging the bottom-right resize handle to `pointer`.
 *
 * The pointer is converted into the stamp's unrotated local space. The stamp
 * grows symmetrically around its centre, so rotation stays visually stable
 * while only width and height change.
 */
export function resizeStamp(
  frame: StampFrameView,
  pointer: Point,
): StampFrameView {
  const center = stampCenter(frame);
  const radians = (frame.rotation * Math.PI) / 180;
  const cos = Math.cos(radians);
  const sin = Math.sin(radians);
  const dx = pointer.x - center.x;
  const dy = pointer.y - center.y;
  const localX = dx * cos + dy * sin;
  const localY = -dx * sin + dy * cos;
  const width = Math.max(localX * 2, MIN_STAMP_WIDTH);
  const height = Math.max(localY * 2, MIN_STAMP_HEIGHT);
  return {
    ...frame,
    x: center.x - width / 2,
    y: center.y - height / 2,
    width,
    height,
  };
}

/**
 * Clockwise rotation from the stamp's centre to the rotate handle, in degrees.
 *
 * The handle sits above the top edge at rotation 0, so the reported angle is
 * measured from "up" rather than from the positive x axis. `snapDegrees` (for
 * example 15 while Shift is held) rounds to that step.
 */
export function rotationFromPointer(
  frame: StampFrameView,
  pointer: Point,
  snapDegrees = 0,
): number {
  const center = stampCenter(frame);
  const degrees =
    (Math.atan2(pointer.y - center.y, pointer.x - center.x) * 180) / Math.PI +
    90;
  const snapped =
    snapDegrees > 0
      ? Math.round(degrees / snapDegrees) * snapDegrees
      : degrees;
  return ((snapped % 360) + 360) % 360;
}

/** Keeps a frame inside the page after a resize or rotation. */
export function fitStampToPage(
  frame: StampFrameView,
  pageWidth: number,
  pageHeight: number,
): StampFrameView {
  return {
    ...frame,
    x: clamp(frame.x, 0, pageWidth - frame.width),
    y: clamp(frame.y, 0, pageHeight - frame.height),
  };
}
