import { describe, expect, it } from "vitest";

import type { TextCharView } from "./backend";
import {
  buildTextLayer,
  editableTextBlocks,
  findCharAt,
  selectionForChar,
  selectionForDrag,
  selectionForLine,
  selectionForWord,
  selectionRects,
  selectionText,
} from "./textLayer";

function makeChar(
  text: string,
  x: number,
  y: number,
  width: number,
  height = 12,
  fontSize = 18,
): TextCharView {
  return {
    text,
    x,
    y,
    width,
    height,
    originX: x,
    originY: y + height,
    fontSize,
    generated: false,
  };
}

function generated(text: string, x: number, y: number): TextCharView {
  return {
    text,
    x,
    y,
    width: 0,
    height: 0,
    originX: x,
    originY: y,
    fontSize: 18,
    generated: true,
  };
}

/** "Hi the" on the first line, then an explicit PDFium break, then "中文". */
const twoLines = buildTextLayer([
  makeChar("H", 10, 10, 8),
  makeChar("i", 18, 10, 4),
  makeChar(" ", 22, 10, 3),
  makeChar("t", 25, 10, 4),
  makeChar("h", 29, 10, 6),
  makeChar("e", 35, 10, 6),
  generated("\r", 41, 10),
  generated("\n", 41, 10),
  makeChar("中", 10, 40, 18, 18),
  makeChar("文", 28, 40, 18, 18),
]);

const layer = {
  pageIndex: 0,
  pageWidth: 100,
  pageHeight: 100,
  rotation: 0,
  cropX: 0,
  cropY: 0,
  hasText: true,
  text: "Hi the\r\n中文",
  chars: [
    makeChar("H", 10, 10, 8),
    makeChar("i", 18, 10, 4),
    makeChar(" ", 22, 10, 3),
    makeChar("t", 25, 10, 4),
    makeChar("h", 29, 10, 6),
    makeChar("e", 35, 10, 6),
    generated("\r", 41, 10),
    generated("\n", 41, 10),
    makeChar("中", 10, 40, 18, 18),
    makeChar("文", 28, 40, 18, 18),
  ],
};

describe("text layer grouping", () => {
  it("uses generated PDFium line breaks and spaces to build lines and words", () => {
    expect(twoLines.lines.map((line) => line.text)).toEqual(["Hi the", "中文"]);
    expect(twoLines.words.map((word) => word.text)).toEqual(["Hi", "the", "中", "文"]);
  });

  it("keeps line ranges in raw character indices and excludes breaks", () => {
    expect(twoLines.lines[0]).toMatchObject({ start: 0, end: 6 });
    expect(twoLines.lines[1]).toMatchObject({ start: 8, end: 10 });
  });

  it("splits words on a visual gap when PDFium emits no space character", () => {
    const structure = buildTextLayer([
      makeChar("a", 10, 10, 8),
      makeChar("b", 18, 10, 8),
      makeChar("c", 40, 10, 8),
      makeChar("d", 48, 10, 8),
    ]);

    expect(structure.words.map((word) => word.text)).toEqual(["ab", "cd"]);
  });

  it("geometrically groups a new line when no generated break exists", () => {
    const structure = buildTextLayer([
      makeChar("a", 10, 10, 8),
      makeChar("b", 18, 10, 8),
      makeChar("c", 10, 40, 8),
      makeChar("d", 18, 40, 8),
    ]);

    expect(structure.lines.map((line) => line.text)).toEqual(["ab", "cd"]);
  });

  it("groups vertical text into lines and words", () => {
    const structure = buildTextLayer([
      makeChar("A", 100, 10, 12),
      makeChar("B", 100, 25, 12),
      makeChar("C", 100, 40, 12),
      makeChar("D", 130, 10, 12),
      makeChar("E", 130, 25, 12),
    ]);

    expect(structure.lines.map((line) => line.text)).toEqual(["ABC", "DE"]);
    expect(structure.words.map((word) => word.text)).toEqual(["ABC", "DE"]);
  });

  it("handles an empty text layer without inventing content", () => {
    const empty = buildTextLayer([]);

    expect(empty.lines).toEqual([]);
    expect(empty.words).toEqual([]);
    expect(selectionText(layer, { start: 0, end: 0 })).toBe("");
  });
});

describe("editable text blocks", () => {
  it("joins adjacent characters on the same baseline into one block", () => {
    const blocks = editableTextBlocks([
      makeChar("业", 10, 10, 18, 18),
      makeChar("务", 28, 10, 18, 18),
    ]);

    expect(blocks).toEqual([{ x: 10, y: 10, width: 36, height: 18 }]);
  });

  it("splits blocks at gaps wider than a space, such as table cells", () => {
    const blocks = editableTextBlocks([
      makeChar("名", 10, 10, 18, 18),
      makeChar("称", 28, 10, 18, 18),
      makeChar("值", 70, 10, 18, 18),
      makeChar("一", 88, 10, 18, 18),
    ]);

    expect(blocks).toEqual([
      { x: 10, y: 10, width: 36, height: 18 },
      { x: 70, y: 10, width: 36, height: 18 },
    ]);
  });

  it("starts a new block after a generated line break", () => {
    const blocks = editableTextBlocks([
      makeChar("上", 10, 10, 18, 18),
      generated("\r", 28, 10),
      generated("\n", 28, 10),
      makeChar("下", 10, 40, 18, 18),
    ]);

    expect(blocks).toHaveLength(2);
    expect(blocks[1]).toEqual({ x: 10, y: 40, width: 18, height: 18 });
  });

  it("splits blocks when the font size changes on one line", () => {
    const blocks = editableTextBlocks([
      makeChar("A", 10, 10, 10, 12, 18),
      makeChar("B", 20, 10, 10, 12, 24),
    ]);

    expect(blocks).toHaveLength(2);
  });

  it("gives each rotated character its own block", () => {
    const blocks = editableTextBlocks([
      makeChar("A", 100, 10, 12),
      makeChar("B", 100, 25, 12),
    ]);

    expect(blocks).toEqual([
      { x: 100, y: 10, width: 12, height: 12 },
      { x: 100, y: 25, width: 12, height: 12 },
    ]);
  });
});

describe("text layer hit testing", () => {
  it("finds the character under a point", () => {
    expect(findCharAt(layer, { x: 11, y: 14 })).toBe(0);
    expect(findCharAt(layer, { x: 36, y: 15 })).toBe(5);
    expect(findCharAt(layer, { x: 12, y: 44 })).toBe(8);
  });

  it("snaps to a nearby character but not to distant empty space", () => {
    expect(findCharAt(layer, { x: 10, y: 24 })).toBe(0);
    expect(findCharAt(layer, { x: 80, y: 90 })).toBeNull();
  });

  it("never hit-tests generated line breaks", () => {
    expect(findCharAt(layer, { x: 41, y: 10 }, 1)).not.toBe(6);
    expect(findCharAt(layer, { x: 41, y: 10 }, 1)).not.toBe(7);
  });
});

describe("text selections", () => {
  it("selects one character, one word, or one line", () => {
    expect(selectionForChar(4)).toEqual({ start: 4, end: 5 });
    expect(selectionForWord(twoLines, 4)).toEqual({ start: 3, end: 6 });
    expect(selectionForWord(twoLines, 9)).toEqual({ start: 9, end: 10 });
    expect(selectionForLine(twoLines, 9)).toEqual({ start: 8, end: 10 });
    expect(selectionForLine(twoLines, 0)).toEqual({ start: 0, end: 6 });
  });

  it("returns null when a word or line selection has no target", () => {
    expect(selectionForWord(twoLines, 2)).toBeNull();
    expect(selectionForWord(twoLines, 6)).toBeNull();
    expect(selectionForLine(twoLines, 6)).toBeNull();
  });

  it("builds a character range for a drag in either direction", () => {
    expect(selectionForDrag(0, 5)).toEqual({ start: 0, end: 6 });
    expect(selectionForDrag(5, 0)).toEqual({ start: 0, end: 6 });
    expect(selectionForDrag(4, 4)).toEqual({ start: 4, end: 5 });
  });

  it("copies selected text with normalized line breaks", () => {
    expect(selectionText(layer, { start: 0, end: 6 })).toBe("Hi the");
    expect(selectionText(layer, { start: 0, end: 10 })).toBe("Hi the\n中文");
    expect(selectionText(layer, { start: 8, end: 10 })).toBe("中文");
  });

  it("merges highlights into one rectangle per selected line", () => {
    const rects = selectionRects(twoLines, { start: 0, end: 10 });

    expect(rects).toHaveLength(2);
    expect(rects[0]).toEqual({ x: 10, y: 10, width: 31, height: 12 });
    expect(rects[1]).toEqual({ x: 10, y: 40, width: 36, height: 18 });
  });

  it("highlights only the selected characters within a line", () => {
    const rects = selectionRects(twoLines, { start: 3, end: 6 });

    expect(rects).toEqual([{ x: 25, y: 10, width: 16, height: 12 }]);
  });
});
