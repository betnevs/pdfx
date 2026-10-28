import { useMemo, type PointerEvent as ReactPointerEvent } from "react";

import type {
  TextAlignment,
  TextBoxFontView,
  TextBoxStyleView,
  TextBoxTargetView,
  TextRangeView,
} from "../lib/backend";

export interface TextBoxStylePanelProps {
  target: TextBoxTargetView;
  fonts: TextBoxFontView[];
  busy: boolean;
  /**
   * Selection in the open Text Box editor, in Unicode code points.
   *
   * `null` means the editor's caret is collapsed, so a change styles the whole
   * box. Outside the editor the panel always applies styles to the whole box.
   */
  selection?: TextRangeView | null;
  /** True while the inline editor is open; then empty selections style the box. */
  editing?: boolean;
  onChange: (style: TextBoxStyleView, selection: TextRangeView | null) => void;
  onDelete: () => void;
}

function wholeBoxStyle(target: TextBoxTargetView): TextBoxStyleView {
  return {
    fontFamily: target.fontFamily,
    fontSize: target.fontSize,
    bold: target.bold,
    italic: target.italic,
    underline: target.underline,
    color: target.color,
    alignment: target.alignment,
  };
}

/**
 * The style of the run at the start of the selection.
 *
 * A selection can cover several runs; the controls then show the style of its
 * first character and a note says so, instead of pretending the range is
 * uniform. Text outside every run uses the box's base style.
 */
function selectionStyle(
  target: TextBoxTargetView,
  selection: TextRangeView,
): TextBoxStyleView {
  const run = target.runs.find(
    (candidate) =>
      candidate.start <= selection.start && selection.start < candidate.end,
  );
  if (!run) {
    return wholeBoxStyle(target);
  }
  return {
    fontFamily: run.fontFamily,
    fontSize: run.fontSize,
    bold: run.bold,
    italic: run.italic,
    underline: run.underline,
    color: run.color,
    alignment: target.alignment,
  };
}

/**
 * Whole-box and selection style controls for a Text Box.
 *
 * Only fonts advertised by the backend's registry are offered: the PDF base-14
 * Latin families and the bundled Noto Sans SC CJK family. Existing text keeps
 * its original embedded font when ttf-parser proves glyph coverage; otherwise
 * the backend falls back to the bundled font and reports it, which the canvas
 * surfaces as a notice.
 *
 * With a non-empty editor selection the controls style only that range and
 * hide the box-level alignment and deletion actions. An empty selection or a
 * box selected outside the editor styles the whole box.
 */
export function TextBoxStylePanel({
  target,
  fonts,
  busy,
  selection = null,
  editing = false,
  onChange,
  onDelete,
}: TextBoxStylePanelProps) {
  const families = useMemo(() => {
    const seen = new Set<string>();
    const result: string[] = [];
    for (const font of fonts) {
      if (!seen.has(font.family)) {
        seen.add(font.family);
        result.push(font.family);
      }
    }
    return result;
  }, [fonts]);

  const baseStyle = selection
    ? selectionStyle(target, selection)
    : wholeBoxStyle(target);
  // A legacy Text Object can carry a font family outside the built-in list
  // (an embedded font, ArialMT, Helvetica-Bold...). The backend only accepts
  // the advertised families for a restyle, so start from an offered family
  // instead of sending one the panel cannot show.
  const selectedFamily = families.includes(baseStyle.fontFamily)
    ? baseStyle.fontFamily
    : (families[0] ?? baseStyle.fontFamily);
  const style: TextBoxStyleView = {
    ...baseStyle,
    fontFamily: selectedFamily,
  };
  const selectionCount = selection ? selection.end - selection.start : 0;
  const touchedRuns = selection
    ? target.runs.filter(
        (run) => run.start < selection.end && run.end > selection.start,
      )
    : [];
  const mixedSelection =
    selection !== null &&
    (touchedRuns.length > 1 ||
      (touchedRuns.length === 1 &&
        (touchedRuns[0].start > selection.start ||
          touchedRuns[0].end < selection.end)));

  const update = (patch: Partial<TextBoxStyleView>) => {
    const next = { ...style, ...patch };
    onChange(next, selection);
  };

  // A button click must not blur the textarea and collapse the selection the
  // change applies to. Selects and colour inputs need focus, so the canvas
  // remembers the range and restores it after the command instead.
  const keepTextSelection = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (selection) {
      event.preventDefault();
    }
  };

  const alignments: Array<{ value: TextAlignment; label: string }> = [
    { value: "left", label: "左对齐" },
    { value: "center", label: "居中" },
    { value: "right", label: "右对齐" },
  ];

  return (
    <div
      className="text-box-style-panel"
      role="region"
      aria-label="文本框样式"
      data-testid="text-box-style-panel"
      onPointerDown={(event) => event.stopPropagation()}
    >
      {editing && (
        <p className="text-box-selection-note">
          {selection
            ? `已选择 ${selectionCount} 个字符，样式更改只应用到所选范围。`
            : "未选择文字；样式更改将应用到整个文本框。"}
        </p>
      )}
      {mixedSelection && (
        <p className="text-box-selection-note">
          所选范围包含多种样式；控件显示起始字符的样式。
        </p>
      )}

      <label className="text-box-style-field">
        <span>字体</span>
        <select
          aria-label="字体"
          value={style.fontFamily}
          disabled={busy}
          onChange={(event) => update({ fontFamily: event.target.value })}
        >
          {families.map((family) => (
            <option key={family} value={family}>
              {family}
            </option>
          ))}
        </select>
      </label>

      <label className="text-box-style-field">
        <span>字号</span>
        <input
          type="number"
          aria-label="字号"
          min={4}
          max={400}
          step={1}
          value={style.fontSize}
          disabled={busy}
          onChange={(event) => {
            const size = Number(event.target.value);
            if (Number.isFinite(size) && size >= 4 && size <= 400) {
              update({ fontSize: size });
            }
          }}
        />
      </label>

      <div className="text-box-style-group" role="group" aria-label="字形">
        <button
          type="button"
          aria-pressed={style.bold}
          disabled={busy}
          onPointerDown={keepTextSelection}
          onClick={() => update({ bold: !style.bold })}
        >
          粗体
        </button>
        <button
          type="button"
          aria-pressed={style.italic}
          disabled={busy}
          onPointerDown={keepTextSelection}
          onClick={() => update({ italic: !style.italic })}
        >
          斜体
        </button>
        <button
          type="button"
          aria-pressed={style.underline}
          disabled={busy}
          onPointerDown={keepTextSelection}
          onClick={() => update({ underline: !style.underline })}
        >
          下划线
        </button>
      </div>

      <label className="text-box-style-field">
        <span>颜色</span>
        <input
          type="color"
          aria-label="颜色"
          value={style.color}
          disabled={busy}
          onChange={(event) => update({ color: event.target.value })}
        />
      </label>

      {!selection && (
        <div className="text-box-style-group" role="group" aria-label="对齐">
          {alignments.map((alignment) => (
            <button
              key={alignment.value}
              type="button"
              aria-pressed={target.alignment === alignment.value}
              disabled={busy}
              onClick={() => update({ alignment: alignment.value })}
            >
              {alignment.label}
            </button>
          ))}
        </div>
      )}

      <p className="text-box-font-note">
        支持 PDF 基础字体（Helvetica、Times、Courier，Latin 字符）和内置中文字体
        Noto Sans SC（中文与 Latin 混排）。编辑已有文字时优先保留原字体；原字体缺少所需字形时自动改用
        Noto Sans SC，并在画布上提示。
      </p>

      {!selection && !editing && (
        <button
          type="button"
          className="text-box-delete"
          disabled={busy}
          onClick={onDelete}
        >
          删除文本框
        </button>
      )}
    </div>
  );
}
