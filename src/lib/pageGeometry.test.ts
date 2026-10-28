import { describe, expect, it } from "vitest";

import {
  anchoredScroll,
  clampScale,
  DEFAULT_SCALE,
  displayPointToPdfPoint,
  fitScale,
  FULL_PAGE_RENDER_PIXEL_LIMIT,
  MAX_SCALE,
  MIN_SCALE,
  pagePixelSize,
  pdfPointToDisplayPoint,
  percentToScale,
  scaleToPercent,
  shouldTile,
  visiblePageRegion,
  ZOOM_STEP,
} from "./pageGeometry";

const letter = { width: 612, height: 792, rotation: 0 as const, cropX: 0, cropY: 0 };
const rotated90 = { width: 792, height: 612, rotation: 90 as const, cropX: 0, cropY: 0 };
const rotated180 = { width: 612, height: 792, rotation: 180 as const, cropX: 0, cropY: 0 };
const rotated270 = { width: 432, height: 288, rotation: 270 as const, cropX: 0, cropY: 0 };

describe("zoom scale limits", () => {
  it("clamps zoom to the 10%-4000% range and rounds float drift", () => {
    expect(MIN_SCALE).toBe(0.1);
    expect(MAX_SCALE).toBe(40);
    expect(clampScale(0.05)).toBe(0.1);
    expect(clampScale(100)).toBe(40);
    expect(clampScale(1.234567)).toBe(1.2346);
    expect(clampScale(Number.NaN)).toBe(DEFAULT_SCALE);
    expect(clampScale(Number.POSITIVE_INFINITY)).toBe(DEFAULT_SCALE);
    expect(ZOOM_STEP).toBeGreaterThan(1);
  });

  it("converts between percent input and scale", () => {
    expect(percentToScale(10)).toBe(0.1);
    expect(percentToScale(4000)).toBe(40);
    expect(percentToScale(5)).toBe(0.1);
    expect(percentToScale(5000)).toBe(40);
    expect(percentToScale(Number.NaN)).toBe(DEFAULT_SCALE);
    expect(scaleToPercent(0.7575)).toBe(76);
    expect(scaleToPercent(40)).toBe(4000);
  });
});

describe("fit modes", () => {
  const viewport = { width: 1000, height: 600 };

  it("fits width to the viewport width", () => {
    expect(fitScale("width", viewport, letter)).toBeCloseTo(1.634, 3);
  });

  it("fits the whole page inside the viewport", () => {
    expect(fitScale("page", viewport, letter)).toBeCloseTo(0.7576, 3);
  });

  it("honours the zoom limits for very large or very small pages", () => {
    expect(fitScale("width", viewport, { width: 100_000, height: 100_000 })).toBe(0.1);
    expect(fitScale("width", viewport, { width: 10, height: 10 })).toBe(40);
  });

  it("falls back to actual size when the viewport has not been measured", () => {
    expect(fitScale("width", { width: 0, height: 0 }, letter)).toBe(DEFAULT_SCALE);
    expect(fitScale("page", viewport, { width: 0, height: 0 })).toBe(DEFAULT_SCALE);
  });
});

describe("page pixel layout", () => {
  it("sizes the page in whole pixels at the requested scale", () => {
    expect(pagePixelSize(letter, 1)).toEqual({ width: 612, height: 792 });
    expect(pagePixelSize(letter, 0.5)).toEqual({ width: 306, height: 396 });
    expect(pagePixelSize(rotated90, 40)).toEqual({ width: 31680, height: 24480 });
    expect(pagePixelSize({ width: 1, height: 1 }, 0.1)).toEqual({ width: 1, height: 1 });
  });

  it("switches to viewport rendering once a full-page bitmap is too large", () => {
    expect(shouldTile(letter, 1)).toBe(false);
    expect(shouldTile(letter, 4)).toBe(false);
    expect(
      letter.width * letter.height * 16 <= FULL_PAGE_RENDER_PIXEL_LIMIT,
    ).toBe(true);
    expect(shouldTile(letter, 20)).toBe(true);
    expect(shouldTile(rotated90, 40)).toBe(true);
  });
});

describe("centered zoom anchoring", () => {
  it("keeps the same page point under the viewport center when zooming in and out", () => {
    expect(anchoredScroll(300, 800, 1, 2)).toBe(1000);
    expect(anchoredScroll(1000, 800, 2, 1)).toBe(300);
  });

  it("never produces a negative scroll offset", () => {
    expect(anchoredScroll(0, 800, 4, 1)).toBe(0);
    expect(anchoredScroll(0, 0, 1, 4)).toBe(0);
  });
});

describe("visible page region for high-zoom rendering", () => {
  it("returns the viewport intersection plus overscan in page pixels", () => {
    const region = visiblePageRegion(
      { scrollLeft: 1000, scrollTop: 500, width: 800, height: 600 },
      { width: 5000, height: 4000 },
      100,
    );

    expect(region).toEqual({ x: 900, y: 400, width: 1000, height: 800 });
  });

  it("clamps to the page bounds", () => {
    expect(
      visiblePageRegion(
        { scrollLeft: 4500, scrollTop: 3800, width: 800, height: 600 },
        { width: 5000, height: 4000 },
        100,
      ),
    ).toEqual({ x: 4400, y: 3700, width: 600, height: 300 });

    expect(
      visiblePageRegion(
        { scrollLeft: 0, scrollTop: 0, width: 900, height: 700 },
        { width: 500, height: 400 },
        100,
      ),
    ).toEqual({ x: 0, y: 0, width: 500, height: 400 });
  });

  it("accounts for a page centered inside a larger viewport", () => {
    // A 500x400 page cannot scroll inside a 900x700 viewport, so the whole page
    // is visible even when the scroll offsets are zero.
    const region = visiblePageRegion(
      { scrollLeft: 0, scrollTop: 0, width: 900, height: 700 },
      { width: 500, height: 400 },
      0,
    );
    expect(region).toEqual({ x: 0, y: 0, width: 500, height: 400 });
  });
});

describe("page coordinate mapping for overlays", () => {
  it("maps unrotated display coordinates to PDF user space", () => {
    expect(displayPointToPdfPoint({ x: 100, y: 200 }, letter, 1)).toEqual({
      x: 100,
      y: 592,
    });
    expect(displayPointToPdfPoint({ x: 200, y: 400 }, letter, 2)).toEqual({
      x: 100,
      y: 592,
    });
  });

  it("maps coordinates through all four rotations", () => {
    // Top-left of the displayed page for each rotation.
    expect(displayPointToPdfPoint({ x: 0, y: 0 }, rotated90, 1)).toEqual({ x: 0, y: 0 });
    expect(displayPointToPdfPoint({ x: 0, y: 0 }, rotated180, 1)).toEqual({
      x: 612,
      y: 792,
    });
    expect(displayPointToPdfPoint({ x: 0, y: 0 }, rotated270, 1)).toEqual({
      x: 288,
      y: 432,
    });

    // Bottom-right of the displayed page for each rotation.
    expect(displayPointToPdfPoint({ x: 792, y: 612 }, rotated90, 1)).toEqual({
      x: 612,
      y: 792,
    });
    expect(displayPointToPdfPoint({ x: 612, y: 792 }, rotated180, 1)).toEqual({
      x: 0,
      y: 0,
    });
    expect(displayPointToPdfPoint({ x: 432, y: 288 }, rotated270, 1)).toEqual({
      x: 0,
      y: 0,
    });
  });

  it("adds the CropBox origin", () => {
    const cropped = { width: 100, height: 200, rotation: 0 as const, cropX: 5, cropY: 7 };
    expect(displayPointToPdfPoint({ x: 0, y: 0 }, cropped, 1)).toEqual({ x: 5, y: 207 });
    expect(displayPointToPdfPoint({ x: 50, y: 100 }, cropped, 2)).toEqual({ x: 30, y: 157 });
  });

  it("round-trips display -> PDF -> display for every rotation", () => {
    const pages = [letter, rotated90, rotated180, rotated270];
    const points = [
      { x: 0, y: 0 },
      { x: 37, y: 81 },
      { x: 250, y: 400 },
    ];

    for (const page of pages) {
      for (const point of points) {
        const pdf = displayPointToPdfPoint(point, page, 1.5);
        const display = pdfPointToDisplayPoint(pdf, page, 1.5);
        expect(display.x).toBeCloseTo(point.x, 6);
        expect(display.y).toBeCloseTo(point.y, 6);
      }
    }
  });
});
