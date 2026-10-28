import type { TextCharView, TextLayerView } from "./backend";

/** A point in display-space page points. */
export interface TextPoint {
  x: number;
  y: number;
}

/** A half-open range of indices into `TextLayerView.chars`. */
export interface TextRange {
  start: number;
  end: number;
}

/** A rectangle in display-space page points. */
export interface TextRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface TextLayerLine {
  start: number;
  end: number;
  text: string;
  x: number;
  y: number;
  width: number;
  height: number;
  /** Raw indices of the characters on this line, in reading order. */
  indices: number[];
  /** Boxes aligned with `indices`. */
  boxes: TextRect[];
}

export interface TextLayerWord {
  start: number;
  end: number;
  text: string;
  lineIndex: number;
  indices: number[];
}

export interface TextLayerStructure {
  lines: TextLayerLine[];
  words: TextLayerWord[];
  lineAtChar: number[];
  wordAtChar: number[];
}

export type TextDirection = "horizontal" | "vertical";

const CJK_CHARACTER = /[぀-ヿ㐀-䶿一-鿿豈-﫿가-힯]/;

function hasBox(character: TextCharView): boolean {
  return !character.generated && character.width > 0 && character.height > 0;
}

function isWhitespace(character: TextCharView): boolean {
  return /^\s+$/.test(character.text);
}

/**
 * Detects the dominant text advance direction from the first two positioned
 * characters. Vertical text occurs when a page is rotated 90 or 270 degrees.
 */
export function detectTextDirection(chars: TextCharView[]): TextDirection {
  let previous: TextCharView | null = null;
  for (const character of chars) {
    if (!hasBox(character)) {
      continue;
    }
    if (previous) {
      const dx = Math.abs(character.originX - previous.originX);
      const dy = Math.abs(character.originY - previous.originY);
      if (dx > 0.01 || dy > 0.01) {
        return dx >= dy ? "horizontal" : "vertical";
      }
    }
    previous = character;
  }
  return "horizontal";
}

interface LineDraft {
  indices: number[];
  left: number;
  top: number;
  right: number;
  bottom: number;
}

function sameLine(draft: LineDraft, character: TextCharView, direction: TextDirection): boolean {
  if (direction === "horizontal") {
    const overlap =
      Math.min(draft.bottom, character.y + character.height) - Math.max(draft.top, character.y);
    return overlap > 0.5 * Math.min(draft.bottom - draft.top, character.height);
  }
  const overlap =
    Math.min(draft.right, character.x + character.width) - Math.max(draft.left, character.x);
  return overlap > 0.5 * Math.min(draft.right - draft.left, character.width);
}

function wordGap(previous: TextCharView, current: TextCharView, direction: TextDirection): number {
  return direction === "horizontal"
    ? current.x - (previous.x + previous.width)
    : current.y - (previous.y + previous.height);
}

function buildWords(
  chars: TextCharView[],
  lineIndices: number[],
  lineIndex: number,
  direction: TextDirection,
  words: TextLayerWord[],
  wordAtChar: number[],
): void {
  let current: { indices: number[]; text: string; previous: TextCharView } | null = null;

  const flush = () => {
    if (!current) {
      return;
    }
    const word: TextLayerWord = {
      start: current.indices[0],
      end: current.indices[current.indices.length - 1] + 1,
      text: current.text,
      lineIndex,
      indices: current.indices,
    };
    const index = words.length;
    words.push(word);
    for (const charIndex of word.indices) {
      wordAtChar[charIndex] = index;
    }
    current = null;
  };

  for (const charIndex of lineIndices) {
    const character = chars[charIndex];
    if (isWhitespace(character)) {
      flush();
      continue;
    }

    // CJK ideographs, kana and hangul have no single-character word boundaries
    // in PDF text, so each one is its own selectable word.
    if (CJK_CHARACTER.test(character.text)) {
      flush();
      const word: TextLayerWord = {
        start: charIndex,
        end: charIndex + 1,
        text: character.text,
        lineIndex,
        indices: [charIndex],
      };
      wordAtChar[charIndex] = words.length;
      words.push(word);
      continue;
    }

    if (current) {
      const gap = wordGap(current.previous, character, direction);
      const threshold = Math.max(2, character.fontSize * 0.4);
      if (gap > threshold) {
        flush();
      }
    }

    if (current) {
      current.indices.push(charIndex);
      current.text += character.text;
      current.previous = character;
    } else {
      current = {
        indices: [charIndex],
        text: character.text,
        previous: character,
      };
    }
  }

  flush();
}

/**
 * Groups PDFium's positioned characters into deterministic visual lines and
 * words. Explicit generated line-break characters win; otherwise a line break
 * is inferred from the text advance direction and box overlap.
 */
export function buildTextLayer(chars: TextCharView[]): TextLayerStructure {
  const direction = detectTextDirection(chars);
  const lines: TextLayerLine[] = [];
  const words: TextLayerWord[] = [];
  const lineAtChar = new Array<number>(chars.length).fill(-1);
  const wordAtChar = new Array<number>(chars.length).fill(-1);
  let draft: LineDraft | null = null;

  const flush = () => {
    if (!draft || draft.indices.length === 0) {
      draft = null;
      return;
    }
    const indices = draft.indices;
    const boxes = indices.map((index) => {
      const character = chars[index];
      return {
        x: character.x,
        y: character.y,
        width: character.width,
        height: character.height,
      };
    });
    const line: TextLayerLine = {
      start: indices[0],
      end: indices[indices.length - 1] + 1,
      text: indices.map((index) => chars[index].text).join(""),
      x: draft.left,
      y: draft.top,
      width: draft.right - draft.left,
      height: draft.bottom - draft.top,
      indices,
      boxes,
    };
    const lineIndex = lines.length;
    for (const index of indices) {
      lineAtChar[index] = lineIndex;
    }
    lines.push(line);
    buildWords(chars, indices, lineIndex, direction, words, wordAtChar);
    draft = null;
  };

  chars.forEach((character, index) => {
    if (character.generated) {
      if (/[\r\n]/.test(character.text)) {
        flush();
      }
      return;
    }
    if (!hasBox(character)) {
      return;
    }
    if (!draft) {
      draft = {
        indices: [index],
        left: character.x,
        top: character.y,
        right: character.x + character.width,
        bottom: character.y + character.height,
      };
      return;
    }
    if (!sameLine(draft, character, direction)) {
      flush();
      draft = {
        indices: [index],
        left: character.x,
        top: character.y,
        right: character.x + character.width,
        bottom: character.y + character.height,
      };
      return;
    }
    draft.indices.push(index);
    draft.left = Math.min(draft.left, character.x);
    draft.top = Math.min(draft.top, character.y);
    draft.right = Math.max(draft.right, character.x + character.width);
    draft.bottom = Math.max(draft.bottom, character.y + character.height);
  });

  flush();

  return { lines, words, lineAtChar, wordAtChar };
}

/**
 * Finds the character under a point, or the nearest one within `tolerance`
 * points. Generated line breaks and zero-sized boxes are never returned.
 */
export function findCharAt(
  layer: TextLayerView,
  point: TextPoint,
  tolerance = 6,
): number | null {
  let contained = -1;
  let containedArea = Number.POSITIVE_INFINITY;
  let nearest = -1;
  let nearestDistance = Number.POSITIVE_INFINITY;

  layer.chars.forEach((character, index) => {
    if (!hasBox(character)) {
      return;
    }
    const dx =
      point.x < character.x
        ? character.x - point.x
        : point.x > character.x + character.width
          ? point.x - (character.x + character.width)
          : 0;
    const dy =
      point.y < character.y
        ? character.y - point.y
        : point.y > character.y + character.height
          ? point.y - (character.y + character.height)
          : 0;
    const distance = Math.hypot(dx, dy);

    if (distance === 0) {
      const area = character.width * character.height;
      if (area < containedArea) {
        contained = index;
        containedArea = area;
      }
    } else if (distance < nearestDistance) {
      nearest = index;
      nearestDistance = distance;
    }
  });

  if (contained !== -1) {
    return contained;
  }
  return nearestDistance <= tolerance ? nearest : null;
}

export function selectionForChar(index: number): TextRange {
  return { start: index, end: index + 1 };
}

export function selectionForWord(
  structure: TextLayerStructure,
  charIndex: number,
): TextRange | null {
  const wordIndex = structure.wordAtChar[charIndex] ?? -1;
  if (wordIndex < 0) {
    return null;
  }
  const word = structure.words[wordIndex];
  return { start: word.start, end: word.end };
}

export function selectionForLine(
  structure: TextLayerStructure,
  charIndex: number,
): TextRange | null {
  const lineIndex = structure.lineAtChar[charIndex] ?? -1;
  if (lineIndex < 0) {
    return null;
  }
  const line = structure.lines[lineIndex];
  return { start: line.start, end: line.end };
}

/** Builds the character range between a drag anchor and its focus. */
export function selectionForDrag(anchor: number, focus: number): TextRange {
  return {
    start: Math.min(anchor, focus),
    end: Math.max(anchor, focus) + 1,
  };
}

/**
 * Copy text for a selection. PDFium's generated `\r` and `\n` characters are
 * collapsed to a single newline so copied multi-line text is clean.
 */
export function selectionText(layer: TextLayerView, selection: TextRange): string {
  const start = Math.max(0, selection.start);
  const end = Math.min(layer.chars.length, selection.end);
  if (end <= start) {
    return "";
  }

  const parts: string[] = [];
  for (let index = start; index < end; index += 1) {
    const text = layer.chars[index].text;
    if (text === "\r" || text === "\n") {
      if (parts[parts.length - 1] !== "\n") {
        parts.push("\n");
      }
    } else {
      parts.push(text);
    }
  }
  return parts.join("");
}

/**
 * Highlight rectangles for a selection, merged into one rectangle per line so
 * dragging across a line produces a single continuous highlight.
 */
export function selectionRects(
  structure: TextLayerStructure,
  selection: TextRange,
): TextRect[] {
  const rects: TextRect[] = [];

  for (const line of structure.lines) {
    let left = Number.POSITIVE_INFINITY;
    let top = Number.POSITIVE_INFINITY;
    let right = Number.NEGATIVE_INFINITY;
    let bottom = Number.NEGATIVE_INFINITY;
    let selected = false;

    line.indices.forEach((index, position) => {
      if (index < selection.start || index >= selection.end) {
        return;
      }
      const box = line.boxes[position];
      selected = true;
      left = Math.min(left, box.x);
      top = Math.min(top, box.y);
      right = Math.max(right, box.x + box.width);
      bottom = Math.max(bottom, box.y + box.height);
    });

    if (selected) {
      rects.push({ x: left, y: top, width: right - left, height: bottom - top });
    }
  }

  return rects;
}
