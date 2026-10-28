import { describe, expect, it } from "vitest";

import type { StampFrameView } from "./backend";
import {
  DEFAULT_STAMP_HEIGHT,
  DEFAULT_STAMP_WIDTH,
  MIN_STAMP_HEIGHT,
  MIN_STAMP_WIDTH,
  moveStamp,
  placeStamp,
  resizeStamp,
  rotationFromPointer,
} from "./stampGeometry";

function frame(
  x: number,
  y: number,
  width: number,
  height: number,
  rotation = 0,
): StampFrameView {
  return { x, y, width, height, rotation };
}

describe("stampGeometry", () => {
  it("places a default-sized stamp centred on the click and inside the page", () => {
    expect(placeStamp({ x: 300, y: 400 }, 612, 792)).toEqual({
      x: 300 - DEFAULT_STAMP_WIDTH / 2,
      y: 400 - DEFAULT_STAMP_HEIGHT / 2,
      width: DEFAULT_STAMP_WIDTH,
      height: DEFAULT_STAMP_HEIGHT,
      rotation: 0,
    });
    expect(placeStamp({ x: 2, y: 2 }, 612, 792)).toEqual({
      x: 0,
      y: 0,
      width: DEFAULT_STAMP_WIDTH,
      height: DEFAULT_STAMP_HEIGHT,
      rotation: 0,
    });
    expect(placeStamp({ x: 611, y: 791 }, 150, 200)).toEqual({
      x: 0,
      y: 200 - DEFAULT_STAMP_HEIGHT,
      width: 150,
      height: DEFAULT_STAMP_HEIGHT,
      rotation: 0,
    });
  });

  it("moves without changing size or rotation and clamps to the page", () => {
    const moved = moveStamp(frame(100, 100, 180, 60, 20), { x: 50, y: -30 }, 612, 792);
    expect(moved).toEqual(frame(150, 70, 180, 60, 20));
    expect(moveStamp(frame(0, 0, 180, 60), { x: -10, y: -10 }, 612, 792)).toEqual(
      frame(0, 0, 180, 60),
    );
    expect(
      moveStamp(frame(500, 760, 180, 60), { x: 100, y: 100 }, 612, 792),
    ).toEqual(frame(432, 732, 180, 60));
  });

  it("resizes symmetrically around the centre in the stamp's local space", () => {
    const original = frame(200, 300, 180, 60, 0);
    expect(resizeStamp(original, { x: 380, y: 360 })).toEqual(
      frame(200, 300, 180, 60, 0),
    );
    expect(resizeStamp(original, { x: 420, y: 400 })).toEqual(
      frame(160, 260, 260, 140, 0),
    );
    expect(resizeStamp(original, { x: 290, y: 330 })).toEqual(
      frame(275, 325, MIN_STAMP_WIDTH, MIN_STAMP_HEIGHT, 0),
    );
  });

  it("follows a rotated stamp's own axes when resizing", () => {
    const original = frame(200, 300, 180, 60, 90);
    // The local +x axis points down and local +y points left after 90 degrees.
    const resized = resizeStamp(original, { x: 290 - 60, y: 330 + 90 });
    expect(resized.width).toBeCloseTo(180, 5);
    expect(resized.height).toBeCloseTo(120, 5);
    expect(resized.rotation).toBe(90);
    expect(resized.x).toBeCloseTo(290 - 90, 5);
    expect(resized.y).toBeCloseTo(330 - 60, 5);
  });

  it("measures rotation from the handle above the stamp and snaps when asked", () => {
    const original = frame(200, 300, 180, 60, 0);
    // Centre is (290, 330); directly above is the unrotated handle position.
    expect(rotationFromPointer(original, { x: 290, y: 100 })).toBeCloseTo(0, 5);
    expect(rotationFromPointer(original, { x: 390, y: 330 })).toBeCloseTo(90, 5);
    expect(
      rotationFromPointer(original, { x: 390, y: 430 }, 15),
    ).toBeCloseTo(135, 5);
    expect(
      rotationFromPointer(original, { x: 290 + 50, y: 330 - 86.6 }),
    ).toBeCloseTo(30, 0);
  });
});
