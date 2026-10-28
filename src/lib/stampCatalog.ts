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
  if (annotation.kind === "stamp") {
    const kind = annotation.stamp?.kind;
    return kind ? `图章 · ${STAMP_LABELS[kind]}` : "图章";
  }
  switch (annotation.kind) {
    case "highlight":
      return "高亮";
    case "underline":
      return "下划线";
    case "strikeout":
      return "删除线";
  }
}
