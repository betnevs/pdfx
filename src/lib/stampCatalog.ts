import type { AnnotationView, StampKind } from "./backend";

/** The built-in stamps, in the order the stamp panel offers them. */
export const STAMP_KINDS: readonly StampKind[] = [
  "approved",
  "draft",
  "confidential",
  "rejected",
];

/** Chinese label for each built-in stamp. */
export const STAMP_LABELS: Record<StampKind, string> = {
  approved: "已批准",
  draft: "草稿",
  confidential: "机密",
  rejected: "已拒绝",
};

/** Ink colour for each built-in stamp, matching the PDF appearance. */
export const STAMP_COLORS: Record<StampKind, string> = {
  approved: "#15803d",
  draft: "#475569",
  confidential: "#b91c1c",
  rejected: "#7f1d1d",
};

/** Readable label for one annotation in the document-wide list. */
export function annotationLabel(annotation: AnnotationView): string {
  switch (annotation.kind) {
    case "highlight":
      return "高亮";
    case "underline":
      return "下划线";
    case "strikeout":
      return "删除线";
    case "squiggly":
      return "波浪线";
    case "square":
      return "矩形";
    case "circle":
      return "椭圆";
    case "note":
      return "便签";
    case "ink":
      return "手绘";
    case "stamp": {
      const kind = annotation.stamp?.kind;
      return kind ? `图章 · ${STAMP_LABELS[kind]}` : "图章";
    }
    case "freetext":
      return "自由文本";
    case "callout":
      return "注释框";
  }
}
