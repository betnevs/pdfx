import { describe, expect, it } from "vitest";

import {
  DEFAULT_BOX_HEIGHT,
  DEFAULT_BOX_WIDTH,
  dragRect,
  estimatedContentHeight,
  moveBox,
  resizeBox,
  resizeBoxFromHandle,
  resizeBoxFromDrag,
  contentSelectionFrame,
  rotationFromPointer,
  wrappedLineCount,
} from "./textBoxGeometry";

describe("textBoxGeometry", () => {
  it("turns a drag into a normalized rectangle and a click into a default box", () => {
    expect(dragRect({ x: 10, y: 20 }, { x: 210, y: 80 })).toEqual({
      x: 10,
      y: 20,
      width: 200,
      height: 60,
    });
    // Dragging up and left normalizes to the same rectangle.
    expect(dragRect({ x: 210, y: 80 }, { x: 10, y: 20 })).toEqual({
      x: 10,
      y: 20,
      width: 200,
      height: 60,
    });
    expect(dragRect({ x: 10, y: 20 }, { x: 12, y: 22 })).toEqual({
      x: 10,
      y: 20,
      width: DEFAULT_BOX_WIDTH,
      height: DEFAULT_BOX_HEIGHT,
    });

    expect(
      moveBox(
        { x: 10, y: 20, width: 100, height: 40 },
        { x: 50, y: 30 },
        612,
        792,
      ),
    ).toEqual({ x: 60, y: 50, width: 100, height: 40 });
    // Moving stops at the page edge without changing the size.
    expect(
      moveBox(
        { x: 500, y: 700, width: 100, height: 40 },
        { x: 200, y: 200 },
        612,
        792,
      ),
    ).toEqual({ x: 512, y: 752, width: 100, height: 40 });
  });

  it("resizes along a rotated box's own axes", () => {
    // A box rotated 90 degrees clockwise. The screen point (-100, 270)
    // relative to its top-left is local (250, 110) inside the box.
    const resized = resizeBox({ x: 10, y: 20, width: 200, height: 60 }, 90, {
      x: -100,
      y: 270,
    });
    expect(resized.x).toBe(10);
    expect(resized.y).toBe(20);
    expect(resized.width).toBeCloseTo(250, 10);
    expect(resized.height).toBeCloseTo(110, 10);
  });

  it("measures rotation clockwise from the top-left corner and snaps on demand", () => {
    const box = { x: 10, y: 20, width: 200, height: 60 };
    expect(rotationFromPointer(box, { x: 10, y: 120 })).toBe(90);
    expect(rotationFromPointer(box, { x: 110, y: 20 })).toBe(0);
    expect(rotationFromPointer(box, { x: 10, y: -80 })).toBe(270);
    // 20 degrees snaps to 15; 100 degrees snaps to 105.
    expect(
      rotationFromPointer(
        box,
        { x: 110, y: 20 + 100 * Math.tan((20 * Math.PI) / 180) },
        15,
      ),
    ).toBe(15);
    expect(
      rotationFromPointer(
        box,
        {
          x: 10 + 100 * Math.cos((100 * Math.PI) / 180),
          y: 20 + 100 * Math.sin((100 * Math.PI) / 180),
        },
        15,
      ),
    ).toBe(105);
  });

  it("estimates wrapped height from explicit newlines and font width", () => {
    expect(wrappedLineCount("", 200, 10, "Helvetica")).toBe(1);
    expect(wrappedLineCount("one\ntwo", 200, 10, "Helvetica")).toBe(2);
    // Helvetica 10pt: each "aaaa" is about 20.8pt wide; at 50pt two words
    // fit on a line (44.4pt) but a third does not.
    expect(wrappedLineCount("aaaa aaaa aaaa", 50, 10, "Helvetica")).toBe(2);
    // Courier is wider, so the same words need more lines.
    expect(wrappedLineCount("aaaa aaaa", 50, 10, "Courier")).toBe(2);
    expect(wrappedLineCount("aaaa aaaa", 50, 10, "Helvetica")).toBe(1);
    expect(estimatedContentHeight("one\ntwo", 200, 10, "Helvetica")).toBe(24);
  });

  it("gives a click-created box a visible default size", () => {
    expect(DEFAULT_BOX_WIDTH).toBeGreaterThanOrEqual(100);
    expect(DEFAULT_BOX_HEIGHT).toBeGreaterThanOrEqual(20);
  });
});

describe("eight content resize handles", () => {
  const rect = { x: 100, y: 100, width: 200, height: 60 };
  it("keeps the opposite edge fixed when resizing from left or top", () => {
    expect(resizeBoxFromHandle(rect, 0, { x: 140, y: 130 }, "w")).toEqual({
      x: 140,
      y: 100,
      width: 160,
      height: 60,
    });
    expect(resizeBoxFromHandle(rect, 0, { x: 200, y: 120 }, "n")).toEqual({
      x: 100,
      y: 120,
      width: 200,
      height: 40,
    });
    expect(resizeBoxFromHandle(rect, 0, { x: 150, y: 120 }, "nw")).toEqual({
      x: 150,
      y: 120,
      width: 150,
      height: 40,
    });
  });
  it("resizes in rotated local coordinates without changing the opposite anchor", () => {
    const resized = resizeBoxFromHandle(rect, 90, { x: 70, y: 350 }, "e");
    expect(resized.x).toBeCloseTo(100);
    expect(resized.y).toBeCloseTo(100);
    expect(resized.width).toBeCloseTo(250);
    expect(resized.height).toBeCloseTo(60);
  });
});

describe("constant screen-size content handles",()=>{
  it("pads short lines to leave room for all eight handles without changing content bounds",()=>{
    const rect={x:100,y:100,width:200,height:10};
    const small=contentSelectionFrame(rect,0,0.5);
    const large=contentSelectionFrame(rect,0,2);
    expect(small.height*0.5).toBe(28);
    expect(large.height*2).toBe(32);
    expect(small.y+small.height/2).toBe(105);
    expect(large.y+large.height/2).toBe(105);
  });
  it("uses the drag delta from padded handles, not the absolute pointer position",()=>{
    const rect={x:100,y:100,width:200,height:20};
    expect(resizeBoxFromDrag(rect,0,{x:306,y:110},{x:326,y:110},"e")).toEqual({...rect,width:220});
  });
});
