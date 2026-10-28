import { describe, expect, it } from "vitest";

import {
  imageCorners,
  imageRotationFromPointer,
  isImageOutsidePage,
  moveImage,
  resizeImage,
  type ImageRect,
} from "./imageGeometry";

const rect: ImageRect = { x: 100, y: 100, width: 200, height: 100 };

function close(actual: { x: number; y: number }, expected: { x: number; y: number }) {
  expect(actual.x).toBeCloseTo(expected.x, 5);
  expect(actual.y).toBeCloseTo(expected.y, 5);
}

describe("image geometry", () => {
  it("moves an image by the pointer delta without changing its size", () => {
    expect(moveImage(rect, { x: -30, y: 45 })).toEqual({
      x: 70,
      y: 145,
      width: 200,
      height: 100,
    });
  });

  it("matches CSS rotation around the image center", () => {
    // At rotation 0 the rotate handle sits above the center, so the pointer
    // straight above the center means 0 degrees.
    expect(imageRotationFromPointer(rect, { x: 200, y: 50 })).toBeCloseTo(0, 5);
    // A pointer to the right of the center is a clockwise quarter turn.
    expect(imageRotationFromPointer(rect, { x: 350, y: 150 })).toBeCloseTo(90, 5);
    // Snapping rounds to the requested step.
    expect(
      imageRotationFromPointer(rect, { x: 300, y: 50 }, 15),
    ).toBeCloseTo(45, 5);
  });

  it("resizes around the top-left corner and preserves the aspect ratio", () => {
    // The unrotated bottom-right corner maps back to the original size.
    expect(resizeImage(rect, 0, { x: 300, y: 200 })).toEqual(rect);

    // Halving the distance from the center along the diagonal halves both
    // dimensions and keeps the center in place.
    const half = resizeImage(rect, 0, { x: 250, y: 175 });
    expect(half.width / half.height).toBeCloseTo(2, 5);
    expect(half.width).toBeCloseTo(100, 5);
    expect(half.height).toBeCloseTo(50, 5);
    expect(half.x).toBeCloseTo(150, 5);
    expect(half.y).toBeCloseTo(125, 5);
  });

  it("resizes along the image's own axes when it is rotated", () => {
    // Rotate 90 degrees around the center (200, 150): the local bottom-right
    // corner (100, 50) becomes (-50, 100) on screen, so the pointer at
    // (150, 250) keeps the full size.
    const rotated = resizeImage(rect, 90, { x: 150, y: 250 });
    expect(rotated.width).toBeCloseTo(200, 5);
    expect(rotated.height).toBeCloseTo(100, 5);
    expect(rotated.x).toBeCloseTo(100, 5);
    expect(rotated.y).toBeCloseTo(100, 5);
  });

  it("never resizes below the minimum size", () => {
    const tiny = resizeImage(rect, 0, { x: 100, y: 100 });
    expect(tiny.width).toBeGreaterThanOrEqual(1);
    expect(tiny.height).toBeGreaterThanOrEqual(1);
  });

  it("computes the four rotated corners around the center", () => {
    const corners = imageCorners(rect, 90);
    // Rotating the 200x100 rectangle a quarter turn swaps its extents around
    // the same center (200, 150).
    const xs = corners.map((point) => point.x).sort((a, b) => a - b);
    const ys = corners.map((point) => point.y).sort((a, b) => a - b);
    expect(xs[0]).toBeCloseTo(150, 5);
    expect(xs[3]).toBeCloseTo(250, 5);
    expect(ys[0]).toBeCloseTo(50, 5);
    expect(ys[3]).toBeCloseTo(250, 5);
  });

  it("detects areas outside the page after rotation", () => {
    const inside: ImageRect = { x: 50, y: 50, width: 100, height: 50 };
    expect(isImageOutsidePage(inside, 0, 612, 792)).toBe(false);
    expect(isImageOutsidePage(inside, 45, 612, 792)).toBe(false);
    expect(isImageOutsidePage({ ...inside, x: -20 }, 0, 612, 792)).toBe(true);
    // Rotation pushes a corner past the page edge.
    expect(
      isImageOutsidePage({ x: 5, y: 5, width: 40, height: 40 }, 45, 612, 792),
    ).toBe(true);
  });

  it("keeps corner identity stable", () => {
    close(imageCorners(rect, 0)[0], { x: 100, y: 100 });
    close(imageCorners(rect, 0)[1], { x: 300, y: 100 });
    close(imageCorners(rect, 0)[2], { x: 300, y: 200 });
    close(imageCorners(rect, 0)[3], { x: 100, y: 200 });
  });
});
