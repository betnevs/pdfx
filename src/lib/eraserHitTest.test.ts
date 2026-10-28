import { describe, expect, it } from "vitest";

import type { AnnotationView, ShapeGeometry, StampFrameView } from "./backend";
import { hitTestEraser } from "./eraserHitTest";

function annotation(overrides: Partial<AnnotationView>): AnnotationView {
  return {
    name: "pdfx-1",
    pageIndex: 0,
    kind: "highlight",
    author: null,
    createdAt: null,
    color: "#ffd400",
    opacity: 100,
    lineWidth: null,
    text: null,
    rects: [],
    strokes: [],
    shape: null,
    stamp: null,
    freeText: null,
    ...overrides,
  };
}

function shape(
  name: string,
  kind: "square" | "circle",
  geometry: ShapeGeometry,
): AnnotationView {
  return annotation({
    name,
    kind,
    shape: {
      kind,
      geometry,
      style: {
        fillColor: "#3366cc",
        strokeColor: "#000000",
        opacity: 100,
        lineWidth: 2,
      },
    },
    rects: [
      {
        x: geometry.x,
        y: geometry.y,
        width: geometry.width,
        height: geometry.height,
      },
    ],
  });
}

function stamp(name: string, frame: StampFrameView): AnnotationView {
  return annotation({
    name,
    kind: "stamp",
    stamp: { kind: "approved", frame },
    rects: [
      {
        x: frame.x,
        y: frame.y,
        width: frame.width,
        height: frame.height,
      },
    ],
  });
}

function ink(
  name: string,
  strokes: { x: number; y: number }[][],
  lineWidth = 2,
): AnnotationView {
  return annotation({ name, kind: "ink", lineWidth, strokes });
}

describe("hitTestEraser", () => {
  it("hits a markup annotation inside any of its line rectangles", () => {
    const markup = annotation({
      kind: "squiggly",
      rects: [
        { x: 100, y: 300, width: 80, height: 20 },
        { x: 100, y: 330, width: 60, height: 20 },
      ],
    });
    expect(hitTestEraser([markup], { x: 130, y: 308 }, { radius: 8 })).toEqual({
      annotation: markup,
      strokeIndex: null,
    });
    expect(
      hitTestEraser([markup], { x: 130, y: 340 }, { radius: 8 })?.annotation,
    ).toBe(markup);
    expect(hitTestEraser([markup], { x: 130, y: 360 }, { radius: 8 })).toBeNull();
  });

  it("hits a note icon inside its bounds", () => {
    const note = annotation({
      kind: "note",
      rects: [{ x: 40, y: 40, width: 30, height: 30 }],
    });
    expect(hitTestEraser([note], { x: 55, y: 55 }, { radius: 8 })).toEqual({
      annotation: note,
      strokeIndex: null,
    });
    expect(hitTestEraser([note], { x: 80, y: 55 }, { radius: 8 })).toBeNull();
  });

  it("respects a rotated rectangle's outline rather than its upright bounds", () => {
    const rotated = shape("shape", "square", {
      x: 100,
      y: 100,
      width: 100,
      height: 40,
      rotation: 90,
    });
    // Rotated 90 degrees around (150, 120), the long axis is vertical: the
    // point below the centre is inside, the point to the right is not.
    expect(
      hitTestEraser([rotated], { x: 150, y: 165 }, { radius: 4 })?.annotation,
    ).toBe(rotated);
    expect(
      hitTestEraser([rotated], { x: 195, y: 120 }, { radius: 4 }),
    ).toBeNull();
  });

  it("hits the inscribed ellipse of a circle shape, not its corners", () => {
    const circle = shape("ellipse", "circle", {
      x: 100,
      y: 100,
      width: 100,
      height: 40,
      rotation: 0,
    });
    expect(
      hitTestEraser([circle], { x: 150, y: 120 }, { radius: 2 })?.annotation,
    ).toBe(circle);
    expect(hitTestEraser([circle], { x: 105, y: 105 }, { radius: 2 })).toBeNull();
  });

  it("respects a stamp frame's rotation", () => {
    const rotated = stamp("stamp", {
      x: 100,
      y: 100,
      width: 120,
      height: 40,
      rotation: 90,
    });
    expect(
      hitTestEraser([rotated], { x: 150, y: 180 }, { radius: 2 })?.annotation,
    ).toBe(rotated);
    expect(
      hitTestEraser([rotated], { x: 220, y: 120 }, { radius: 2 }),
    ).toBeNull();
  });

  it("returns the visually topmost annotation when shapes overlap", () => {
    const below = annotation({
      kind: "highlight",
      rects: [{ x: 100, y: 100, width: 100, height: 100 }],
    });
    const above = shape("above", "square", {
      x: 120,
      y: 120,
      width: 40,
      height: 40,
      rotation: 0,
    });
    expect(
      hitTestEraser([below, above], { x: 140, y: 140 }, { radius: 4 })
        ?.annotation,
    ).toBe(above);
  });

  it("hits the nearest ink stroke within the eraser radius", () => {
    const drawing = ink("ink", [
      [
        { x: 100, y: 300 },
        { x: 160, y: 300 },
      ],
      [
        { x: 100, y: 360 },
        { x: 160, y: 360 },
      ],
    ]);
    expect(
      hitTestEraser([drawing], { x: 130, y: 305 }, { radius: 8 }),
    ).toEqual({ annotation: drawing, strokeIndex: 0 });
    expect(
      hitTestEraser([drawing], { x: 130, y: 365 }, { radius: 8 }),
    ).toEqual({ annotation: drawing, strokeIndex: 1 });
    expect(hitTestEraser([drawing], { x: 130, y: 330 }, { radius: 8 })).toBeNull();
  });

  it("adds the ink line width to the eraser radius", () => {
    const drawing = ink(
      "ink",
      [
        [
          { x: 100, y: 300 },
          { x: 160, y: 300 },
        ],
      ],
      20,
    );
    // 12 points away is outside an 8-point radius, but inside half the 20-point
    // stroke plus that radius.
    expect(
      hitTestEraser([drawing], { x: 130, y: 312 }, { radius: 8 })?.strokeIndex,
    ).toBe(0);
    expect(hitTestEraser([drawing], { x: 130, y: 330 }, { radius: 8 })).toBeNull();
  });

  it("deletes a whole ink annotation when wholeInk is requested", () => {
    const drawing = ink("ink", [
      [
        { x: 100, y: 300 },
        { x: 160, y: 300 },
      ],
    ]);
    expect(
      hitTestEraser([drawing], { x: 130, y: 302 }, { radius: 2, wholeInk: true }),
    ).toEqual({ annotation: drawing, strokeIndex: null });
    expect(
      hitTestEraser([drawing], { x: 130, y: 380 }, { radius: 2, wholeInk: true }),
    ).toBeNull();
  });

  it("prefers a whole annotation over a stroke underneath it", () => {
    const drawing = ink("ink", [
      [
        { x: 100, y: 300 },
        { x: 160, y: 300 },
      ],
    ]);
    const note = annotation({
      kind: "note",
      rects: [{ x: 120, y: 285, width: 30, height: 30 }],
    });
    expect(
      hitTestEraser([drawing, note], { x: 130, y: 300 }, { radius: 8 }),
    ).toEqual({ annotation: note, strokeIndex: null });
  });

  it("ignores annotations on another page", () => {
    const other = annotation({
      kind: "note",
      pageIndex: 1,
      rects: [{ x: 40, y: 40, width: 30, height: 30 }],
    });
    expect(
      hitTestEraser([other], { x: 55, y: 55 }, { radius: 8, pageIndex: 0 }),
    ).toBeNull();
  });

  it("handles degenerate geometry without throwing", () => {
    const zero = shape("zero", "square", {
      x: 10,
      y: 10,
      width: 0,
      height: 0,
      rotation: 0,
    });
    const emptyStroke = ink("empty", [[{ x: 50, y: 50 }]]);
    expect(
      hitTestEraser([zero, emptyStroke], { x: 10, y: 10 }, { radius: 4 })
        ?.annotation.name,
    ).toBe("zero");
    expect(hitTestEraser([emptyStroke], { x: 50, y: 50 }, { radius: 4 })).toEqual({
      annotation: emptyStroke,
      strokeIndex: 0,
    });
  });
});
