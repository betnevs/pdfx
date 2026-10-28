import { describe, expect, it } from "vitest";

import {
  MIN_SHAPE_SIZE,
  moveShape,
  resizeShape,
  shapeBounds,
  shapeRectFromDrag,
  shapeRotationFromPointer,
  type ShapeGeometryLike,
} from "./shapeGeometry";

const PAGE_WIDTH = 612;
const PAGE_HEIGHT = 792;

function geometry(
  x: number,
  y: number,
  width: number,
  height: number,
  rotation = 0,
): ShapeGeometryLike {
  return { x, y, width, height, rotation };
}

/** Rotates a local offset around a centre the same way the overlay does. */
function rotatePoint(
  x: number,
  y: number,
  centreX: number,
  centreY: number,
  degrees: number,
) {
  const radians = (degrees * Math.PI) / 180;
  const dx = x - centreX;
  const dy = y - centreY;
  return {
    x: centreX + dx * Math.cos(radians) - dy * Math.sin(radians),
    y: centreY + dx * Math.sin(radians) + dy * Math.cos(radians),
  };
}

describe("shapeRectFromDrag", () => {
  it("normalises a drag in any direction into a positive page rect", () => {
    expect(
      shapeRectFromDrag({ x: 500, y: 700 }, { x: 650, y: 850 }, PAGE_WIDTH, PAGE_HEIGHT),
    ).toEqual({ x: 500, y: 700, width: 112, height: 92 });
    expect(
      shapeRectFromDrag({ x: 200, y: 300 }, { x: 100, y: 250 }, PAGE_WIDTH, PAGE_HEIGHT),
    ).toEqual({ x: 100, y: 250, width: 100, height: 50 });
  });

  it("clamps a drag that leaves the page", () => {
    expect(
      shapeRectFromDrag({ x: -20, y: -10 }, { x: 700, y: 900 }, PAGE_WIDTH, PAGE_HEIGHT),
    ).toEqual({ x: 0, y: 0, width: 612, height: 792 });
  });

  it("rejects clicks and drags too small to be intentional", () => {
    expect(
      shapeRectFromDrag({ x: 100, y: 100 }, { x: 100, y: 100 }, PAGE_WIDTH, PAGE_HEIGHT),
    ).toBeNull();
    expect(
      shapeRectFromDrag({ x: 100, y: 100 }, { x: 101, y: 150 }, PAGE_WIDTH, PAGE_HEIGHT),
    ).toBeNull();
    expect(
      shapeRectFromDrag(
        { x: 100, y: 100 },
        { x: 100 + MIN_SHAPE_SIZE, y: 100 + MIN_SHAPE_SIZE },
        PAGE_WIDTH,
        PAGE_HEIGHT,
      ),
    ).toEqual({ x: 100, y: 100, width: MIN_SHAPE_SIZE, height: MIN_SHAPE_SIZE });
  });
});

describe("shapeBounds", () => {
  it("returns the unrotated rectangle at zero rotation", () => {
    expect(shapeBounds(geometry(10, 20, 30, 40))).toEqual({
      x: 10,
      y: 20,
      width: 30,
      height: 40,
    });
  });

  it("returns the axis-aligned bounds of a rotated shape", () => {
    const bounds = shapeBounds(geometry(100, 100, 100, 60, 90));
    expect(bounds.width).toBeCloseTo(60, 5);
    expect(bounds.height).toBeCloseTo(100, 5);
    expect(bounds.x).toBeCloseTo(120, 5);
    expect(bounds.y).toBeCloseTo(80, 5);
  });
});

describe("moveShape", () => {
  it("translates without changing size or rotation", () => {
    const moved = moveShape(geometry(100, 200, 80, 40, 30), { x: 25, y: -15 }, PAGE_WIDTH, PAGE_HEIGHT);
    expect(moved).toEqual(geometry(125, 185, 80, 40, 30));
  });

  it("keeps the rotated bounding box inside the page", () => {
    const moved = moveShape(geometry(100, 100, 100, 50, 90), { x: 1000, y: 1000 }, PAGE_WIDTH, PAGE_HEIGHT);
    const bounds = shapeBounds(moved);
    expect(bounds.x).toBeCloseTo(PAGE_WIDTH - 50, 5);
    expect(bounds.y).toBeCloseTo(PAGE_HEIGHT - 100, 5);
    expect(moved.rotation).toBe(90);
  });
});

describe("resizeShape", () => {
  it("grows from the fixed top-left corner in the rotated local frame", () => {
    const original = geometry(100, 100, 100, 60, 45);
    const centre = { x: 150, y: 130 };
    const handle = rotatePoint(200, 160, centre.x, centre.y, 45);
    const unchanged = resizeShape(original, handle, PAGE_WIDTH, PAGE_HEIGHT);
    expect(unchanged.width).toBeCloseTo(100, 4);
    expect(unchanged.height).toBeCloseTo(60, 4);

    // Move 20pt along the rotated local x axis: only width grows.
    const localX = { x: Math.cos(Math.PI / 4) * 20, y: Math.sin(Math.PI / 4) * 20 };
    const grown = resizeShape(
      original,
      { x: handle.x + localX.x, y: handle.y + localX.y },
      PAGE_WIDTH,
      PAGE_HEIGHT,
    );
    expect(grown.width).toBeCloseTo(120, 4);
    expect(grown.height).toBeCloseTo(60, 4);
    expect(grown.rotation).toBe(45);

    // The opposite corner stays fixed.
    const fixedBefore = rotatePoint(100, 100, centre.x, centre.y, 45);
    const fixedAfter = rotatePoint(
      grown.x,
      grown.y,
      grown.x + grown.width / 2,
      grown.y + grown.height / 2,
      45,
    );
    expect(fixedAfter.x).toBeCloseTo(fixedBefore.x, 4);
    expect(fixedAfter.y).toBeCloseTo(fixedBefore.y, 4);
  });

  it("never shrinks below the minimum size", () => {
    const original = geometry(100, 100, 100, 60, 0);
    const fixed = { x: 100, y: 100 };
    const resized = resizeShape(original, fixed, PAGE_WIDTH, PAGE_HEIGHT);
    expect(resized.width).toBe(MIN_SHAPE_SIZE);
    expect(resized.height).toBe(MIN_SHAPE_SIZE);
  });
});

describe("shapeRotationFromPointer", () => {
  const centre = { x: 100, y: 100 };

  it("turns the pointer angle delta into clockwise degrees", () => {
    expect(
      shapeRotationFromPointer(
        centre,
        { x: 200, y: 100 },
        { x: 100, y: 200 },
        0,
      ),
    ).toBeCloseTo(90, 5);
  });

  it("keeps the starting rotation and wraps through 360", () => {
    expect(
      shapeRotationFromPointer(
        centre,
        { x: 100, y: 200 },
        { x: 200, y: 100 },
        30,
      ),
    ).toBeCloseTo(300, 5);
  });

  it("snaps to a step while Shift is held", () => {
    const rotation = shapeRotationFromPointer(
      centre,
      { x: 200, y: 100 },
      { x: 150, y: 186.6025 },
      0,
      15,
    );
    expect(rotation % 15).toBe(0);
    expect(rotation).toBe(60);
  });
});
