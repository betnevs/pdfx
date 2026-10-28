import { useMemo } from "react";

import type {
  TextAlignment,
  TextBoxFontView,
  TextBoxStyleView,
  TextBoxTargetView,
} from "../lib/backend";

export interface TextBoxStylePanelProps {
  target: TextBoxTargetView;
  fonts: TextBoxFontView[];
  busy: boolean;
  onChange: (style: TextBoxStyleView) => void;
  onDelete: () => void;
}

/**
 * Whole-box style controls for a selected Text Box.
 *
 * Only fonts advertised by the backend's registry are offered: the PDF base-14
 * Latin families and the bundled Noto Sans SC CJK family. Existing text keeps
 * its original embedded font when ttf-parser proves glyph coverage; otherwise
 * the backend falls back to the bundled font and reports it, which the canvas
 * surfaces as a notice.
 */
export function TextBoxStylePanel({
  target,
  fonts,
  busy,
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

  const style: TextBoxStyleView = {
    fontFamily: target.fontFamily,
    fontSize: target.fontSize,
    bold: target.bold,
    italic: target.italic,
    underline: target.underline,
    color: target.color,
    alignment: target.alignment,
  };

  const update = (patch: Partial<TextBoxStyleView>) =>
    onChange({ ...style, ...patch });

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
      <label className="text-box-style-field">
        <span>字体</span>
        <select
          aria-label="字体"
          value={target.fontFamily}
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
          value={target.fontSize}
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
          aria-pressed={target.bold}
          disabled={busy}
          onClick={() => update({ bold: !target.bold })}
        >
          粗体
        </button>
        <button
          type="button"
          aria-pressed={target.italic}
          disabled={busy}
          onClick={() => update({ italic: !target.italic })}
        >
          斜体
        </button>
        <button
          type="button"
          aria-pressed={target.underline}
          disabled={busy}
          onClick={() => update({ underline: !target.underline })}
        >
          下划线
        </button>
      </div>

      <label className="text-box-style-field">
        <span>颜色</span>
        <input
          type="color"
          aria-label="颜色"
          value={target.color}
          disabled={busy}
          onChange={(event) => update({ color: event.target.value })}
        />
      </label>

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

      <p className="text-box-font-note">
        支持 PDF 基础字体（Helvetica、Times、Courier，Latin 字符）和内置中文字体
        Noto Sans SC（中文与 Latin 混排）。编辑已有文字时优先保留原字体；原字体缺少所需字形时自动改用
        Noto Sans SC，并在画布上提示。
      </p>

      <button
        type="button"
        className="text-box-delete"
        disabled={busy}
        onClick={onDelete}
      >
        删除文本框
      </button>
    </div>
  );
}
