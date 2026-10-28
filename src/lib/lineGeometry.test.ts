import { describe, expect, it } from "vitest";

import {
  arrowHeadPoints,
  lineAngle,
  lineFromDrag,
  lineLength,
  lineMidpoint,
  lineRotationFromPointer,
  moveLine,
  moveLineEndpoint,
  rotateLine,
} from "./lineGeometry";

describe("lineFromDrag", () => {
  it("clamps both endpoints to the page", () => {
    expect(
      lineFromDrag({ x: -20, y: 700 }, { x: 700, y: 900 }, 612, 792),
    ).toEqual({ x1: 0, y1: 700, x2: 612, y2: 792 });
  });

  it("rejects a click or a drag too short to be intentional", () => {
    expect(lineFromDrag({ x: 100, y: 100 }, { x: 100, y: 100 }, 612, 792)).toBeNull();
    expect(lineFromDrag({ x: 100, y: 100 }, { x: 101, y: 100 }, 612, 792)).toBeNull();
  });
});

describe("moveLine", () => {
  it("translates both endpoints and clamps the line inside the page", () => {
    const moved = moveLine(
      { x1: 100, y1: 100, x2: 300, y2: 100 },
      { x: 400, y: -20 },
      612,
      792,
    );
    // The translation is clamped as a whole so the far endpoint stops at the
    // page edge (x2 = 612) while the segment keeps its length and angle.
    expect(moved).toEqual({ x1: 412, y1: 80, x2: 612, y2: 80 });
    expect(lineLength(moved)).toBeCloseTo(200, 6);
  });
});

describe("moveLineEndpoint", () => {
  it("moves only the dragged endpoint", () => {
    expect(
      moveLineEndpoint(
        { x1: 100, y1: 100, x2: 300, y2: 100 },
        "end",
        { x: 250, y: 400 },
        612,
        792,
      ),
    ).toEqual({ x1: 100, y1: 100, x2: 250, y2: 400 });
  });

  it("clamps the dragged endpoint to the page", () => {
    expect(
      moveLineEndpoint(
        { x1: 100, y1: 100, x2: 300, y2: 100 },
        "start",
        { x: -30, y: 900 },
        612,
        792,
      ),
    ).toEqual({ x1: 0, y1: 792, x2: 300, y2: 100 });
  });
});

describe("rotateLine", () => {
  it("rotates both endpoints around the midpoint and keeps the length", () => {
    const rotated = rotateLine({ x1: 100, y1: 100, x2: 300, y2: 100 }, 90);
    expect(rotated.x1).toBeCloseTo(200, 6);
    expect(rotated.y1).toBeCloseTo(0, 6);
    expect(rotated.x2).toBeCloseTo(200, 6);
    expect(rotated.y2).toBeCloseTo(200, 6);
    expect(lineLength(rotated)).toBeCloseTo(200, 6);
  });

  it("reports the angle and midpoint used by the property panel", () => {
    const geometry = { x1: 100, y1: 300, x2: 300, y2: 300 };
    expect(lineMidpoint(geometry)).toEqual({ x: 200, y: 300 });
    expect(lineAngle(geometry)).toBeCloseTo(0, 6);
    expect(
      lineAngle(rotateLine(geometry, 90)),
    ).toBeCloseTo(90, 6);
  });
});

describe("lineRotationFromPointer", () => {
  it("returns the pointer's angular travel around the midpoint", () => {
    const geometry = { x1: 100, y1: 300, x2: 300, y2: 300 };
    // Start above the midpoint (-90 degrees), end to its right (0 degrees).
    const delta = lineRotationFromPointer(
      geometry,
      { x: 200, y: 282 },
      { x: 218, y: 300 },
    );
    expect(delta).toBeCloseTo(90, 6);
  });

  it("snaps the absolute angle while Shift is held", () => {
    const geometry = { x1: 100, y1: 300, x2: 300, y2: 300 };
    const start = { x: 200, y: 282 };
    const radians = (-82 * Math.PI) / 180;
    const current = {
      x: 200 + 10 * Math.cos(radians),
      y: 300 + 10 * Math.sin(radians),
    };
    expect(
      lineRotationFromPointer(geometry, start, current, 0),
    ).toBeCloseTo(8, 6);
    expect(
      lineRotationFromPointer(geometry, start, current, 15),
    ).toBeCloseTo(15, 6);
  });
});

describe("arrowHeadPoints", () => {
  it("matches the backend's head length and half width for a 4pt stroke", () => {
    const [tip, left, right] = arrowHeadPoints(
      { x1: 100, y1: 100, x2: 300, y2: 100 },
      4,
    );
    expect(tip).toEqual({ x: 300, y: 100 });
    expect(left.x).toBeCloseTo(284, 6);
    expect(left.y).toBeCloseTo(105.6, 6);
    expect(right.x).toBeCloseTo(284, 6);
    expect(right.y).toBeCloseTo(94.4, 6);
  });

  it("never grows the head past the line itself", () => {
    const [tip, , right] = arrowHeadPoints(
      { x1: 100, y1: 100, x2: 105, y2: 100 },
      4,
    );
    expect(tip.x).toBeCloseTo(105, 6);
    expect(right.x).toBeCloseTo(100, 6);
  });
});
