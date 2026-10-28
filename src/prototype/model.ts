import { copy } from "./copy";

export type WorkspaceMode = "annotate" | "edit";

export type TabId = "annotate" | "shapes" | "insert" | "editText" | "forms";

export const TAB_ORDER: readonly TabId[] = [
  "annotate",
  "shapes",
  "insert",
  "editText",
  "forms",
];

export const TAB_MODE: Record<TabId, WorkspaceMode> = {
  annotate: "annotate",
  shapes: "annotate",
  insert: "annotate",
  editText: "edit",
  forms: "edit",
};

export const MODE_TAB: Record<WorkspaceMode, TabId> = {
  annotate: "annotate",
  edit: "editText",
};

export type ToolId = keyof typeof copy.tools;

export const TAB_TOOLS: Record<TabId, readonly ToolId[]> = {
  annotate: ["highlight", "underline", "strikeout", "note", "ink"],
  shapes: ["rect", "ellipse", "line", "arrow"],
  insert: ["image", "signature", "link", "stamp"],
  editText: ["editText", "addText"],
  forms: ["formText", "formCheckbox", "formRadio", "formSelect"],
};

export const DEFAULT_TOOL: Record<TabId, ToolId> = {
  annotate: "highlight",
  shapes: "rect",
  insert: "image",
  editText: "editText",
  forms: "formText",
};

export type Selection =
  | { kind: "none" }
  | { kind: "text"; id: string; label: string; page: number }
  | { kind: "annotation"; id: string; label: string; page: number }
  | { kind: "shape"; id: string; label: string; page: number };

export const NO_SELECTION: Selection = { kind: "none" };

export type TextFontId = "sans" | "serif" | "kai";
export type TextAlign = "left" | "center" | "right";

export type TextStyle = {
  fontId: TextFontId;
  fontSize: number;
  color: string;
  bold: boolean;
  italic: boolean;
  underline: boolean;
  align: TextAlign;
};

export type AnnotationStyle = {
  color: string;
  opacity: number;
  note: string;
};

export type ShapeStyle = {
  fill: string;
  stroke: string;
  strokeWidth: number;
  opacity: number;
};

export const FONT_STACKS: Record<TextFontId, string> = {
  sans: '-apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif',
  serif: '"Songti SC", "STSong", "SimSun", Georgia, serif',
  kai: '"Kaiti SC", "STKaiti", "KaiTi", serif',
};

export const TEXT_FONT_IDS: readonly TextFontId[] = ["sans", "serif", "kai"];

export type ColorName = keyof typeof copy.colors;

export type ColorOption = { id: ColorName; value: string };

export const TEXT_COLORS: readonly ColorOption[] = [
  { id: "black", value: "#1f2430" },
  { id: "red", value: "#d93b30" },
  { id: "blue", value: "#2563eb" },
  { id: "green", value: "#1f8a5b" },
];

export const ANNOTATION_COLORS: readonly ColorOption[] = [
  { id: "yellow", value: "#f6c945" },
  { id: "green", value: "#4fbf7a" },
  { id: "blue", value: "#5b9bf5" },
  { id: "pink", value: "#f28bb4" },
  { id: "orange", value: "#f59e42" },
];

export const SHAPE_STROKE_COLORS: readonly ColorOption[] = [
  { id: "blue", value: "#2563eb" },
  { id: "red", value: "#d93b30" },
  { id: "green", value: "#1f8a5b" },
  { id: "slate", value: "#4b5563" },
];

export const SHAPE_FILL_COLORS: readonly ColorOption[] = [
  { id: "none", value: "none" },
  { id: "yellow", value: "#f9e6a3" },
  { id: "blue", value: "#cfe0fb" },
  { id: "green", value: "#cdeedc" },
  { id: "pink", value: "#f7d4e2" },
];

export const ZOOM_MIN = 10;
export const ZOOM_MAX = 4000;
export const ZOOM_STEP = 25;

export type ZoomMode = "custom" | "fit-width" | "fit-page";

export type ViewportSize = { width: number; height: number };

/** Horizontal and vertical padding around the page inside the canvas scroller. */
export const CANVAS_PADDING_X = 64;
export const CANVAS_PADDING_Y = 92;

/** Used before the canvas has been measured (and in jsdom during tests). */
export const FALLBACK_VIEWPORT: ViewportSize = { width: 900, height: 560 };

export const PAGE_WIDTH = 720;
export const PAGE_HEIGHT = 1018;

export function clampZoom(value: number): number {
  if (!Number.isFinite(value)) {
    return 100;
  }
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.round(value)));
}

export function parseZoom(raw: string): number {
  return clampZoom(Number.parseFloat(raw.replace("%", "").trim()));
}

export function fitWidthZoom(viewport: ViewportSize): number {
  return clampZoom(
    ((viewport.width - CANVAS_PADDING_X) / PAGE_WIDTH) * 100,
  );
}

export function fitPageZoom(viewport: ViewportSize): number {
  return clampZoom(
    Math.min(
      (viewport.width - CANVAS_PADDING_X) / PAGE_WIDTH,
      (viewport.height - CANVAS_PADDING_Y) / PAGE_HEIGHT,
    ) * 100,
  );
}

export function clampPage(value: number, pageCount: number): number {
  if (!Number.isFinite(value)) {
    return 1;
  }
  return Math.min(pageCount, Math.max(1, Math.round(value)));
}
