import { useMemo, type PointerEvent as ReactPointerEvent } from "react";

import type {
  TextAlignment,
  TextBoxFontView,
  TextBoxStyleView,
  TextBoxTargetView,
  TextRangeView,
} from "../lib/backend";
import { WorkspaceIcon } from "./WorkspaceIcon";
import { TextColorPalette } from "./TextStylePlaceholder";
import { isFangSongFamily, isSerifFamily } from "../lib/fontPreview";

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
  onLink?: () => void;
  showDelete?: boolean;
}

/** Sizes WPS and Word put in front of the user, in points. */
const FONT_SIZE_PRESETS = [
  8, 9, 10, 10.5, 11, 12, 14, 16, 18, 20, 22, 24, 26, 28, 32, 36, 42, 48, 56,
  64, 72,
];

/**
 * The value of the "原字体" choice.
 *
 * It is a sentinel rather than the original family name so the select keeps
 * showing the original choice even when that family is also installed. The
 * installed duplicate is hidden while this mode is active because both
 * options would carry the same family name and the backend would keep using
 * the embedded program either way.
 */
const KEEP_ORIGINAL_FONT = "__original__";

type FontOption = {
  value: string;
  label: string;
  source: string;
};

function wholeBoxStyle(target: TextBoxTargetView): TextBoxStyleView {
  return {
    fontFamily: target.fontFamily,
    fontSize: target.fontSize,
    bold: target.bold,
    italic: target.italic,
    underline: target.underline,
    strikeout: target.strikeout ?? false,
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
    strikeout: run.strikeout ?? false,
    color: run.color,
    alignment: target.alignment,
  };
}

/** Removes the `ABCDEF+` subset prefix from a PDF font name. */
function baseFontName(family: string): string {
  return /^[A-Z]{6}\+/.test(family) ? family.slice(7) : family;
}

/** The advertised family that reproduces a PDF font name, when there is one. */
function advertisedFamily(
  targetFamily: string,
  fonts: TextBoxFontView[],
): string | null {
  const name = baseFontName(targetFamily);
  const match = fonts.find(
    (font) => font.family === name || font.postscriptName === name,
  );
  return match?.family ?? null;
}

/**
 * The bundled face closest to an original font PDFX cannot reproduce itself.
 *
 * This mirrors the backend's fallback choice, so the panel shows the family
 * the first style change will actually use instead of silently defaulting to
 * Helvetica.
 */
function suggestedFamily(targetFamily: string, families: string[]): string {
  const preferred = isFangSongFamily(targetFamily)
    ? "Zhuque Fangsong"
    : isSerifFamily(targetFamily)
      ? "Noto Serif SC"
      : "Noto Sans SC";
  return families.includes(preferred)
    ? preferred
    : (families[0] ?? targetFamily);
}

/**
 * Whole-box and selection style controls for a Text Box.
 *
 * The font list is the backend's registry: the PDF base-14 Latin families,
 * the bundled OFL CJK faces and the fonts installed on this machine (OS fonts
 * and WPS Office's document fonts). Existing text keeps its original embedded
 * font when ttf-parser proves glyph coverage; a missing character first uses
 * the installed copy of the same family, then the bundled face, and the
 * backend reports the latter on the canvas.
 *
 * A box that still paints an arbitrary embedded font offers that font as an
 * "原字体" choice. Only the size and colour can change in that mode, because
 * any other change needs a replacement font; choosing another family
 * switches the box to a fully styleable Text Box.
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
  onLink,
  showDelete = true,
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

  const labels = useMemo(() => {
    const result = new Map<string, string>();
    for (const font of fonts) {
      if (!result.has(font.family)) {
        result.set(font.family, font.label);
      }
    }
    return result;
  }, [fonts]);

  const sources = useMemo(() => {
    const result = new Map<string, string>();
    for (const font of fonts) {
      if (!result.has(font.family)) {
        result.set(font.family, font.source);
      }
    }
    return result;
  }, [fonts]);

  const baseStyle = selection
    ? selectionStyle(target, selection)
    : wholeBoxStyle(target);
  // A legacy Text Object can carry a font family outside the built-in list
  // (an embedded font, Helvetica-Bold, Times-Roman...). Match it to the
  // advertised variant first, then to the bundled look-alike the backend
  // would fall back to, instead of defaulting every edit to Helvetica.
  const keepOriginal =
    target.keepOriginalFont && baseStyle.fontFamily === target.fontFamily;
  // Prefer the installed family's localized label (for example 仿宋-简) when
  // the same family is on this machine; otherwise show the PDF's font name.
  const originalLabel = labels.get(target.fontFamily) ?? target.fontFamily;
  const advertised = advertisedFamily(baseStyle.fontFamily, fonts);
  const selectedFamily = keepOriginal
    ? target.fontFamily
    : (advertised ?? suggestedFamily(baseStyle.fontFamily, families));
  const style: TextBoxStyleView = {
    ...baseStyle,
    fontFamily: selectedFamily,
  };
  // The "原字体" choice is rendered separately when `keepOriginal` is true, so
  // it stays out of the optgroups and is never listed twice. An installed copy
  // of that same family is hidden too: its option value is the original family
  // name, which cannot ask the backend for the installed program, so choosing
  // it would silently stay on the original font.
  const familyOptions = useMemo<FontOption[]>(
    () =>
      families
        .filter((family) => !(keepOriginal && family === target.fontFamily))
        .map((family) => ({
          value: family,
          label: labels.get(family) ?? family,
          source: sources.get(family) ?? "embedded",
        })),
    [families, labels, sources, keepOriginal, target.fontFamily],
  );
  const builtInOptions = familyOptions.filter(
    (option) => option.source !== "wps" && option.source !== "system",
  );
  const wpsOptions = familyOptions.filter((option) => option.source === "wps");
  const systemOptions = familyOptions.filter(
    (option) => option.source === "system",
  );
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

  // The original-font mode has no replacement program, so the backend refuses
  // weight, underline and alignment changes; disable instead of failing.
  const styleLocked = busy || (keepOriginal && selection !== null);
  const sizePreset = FONT_SIZE_PRESETS.includes(style.fontSize)
    ? String(style.fontSize)
    : "";

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
        <p className="text-box-selection-note visually-hidden">
          {selection
            ? `已选择 ${selectionCount} 个字符，样式更改只应用到所选范围。`
            : "未选择文字；样式更改将应用到整个文本框。"}
        </p>
      )}
      {mixedSelection && (
        <p className="text-box-selection-note visually-hidden">
          所选范围包含多种样式；控件显示起始字符的样式。
        </p>
      )}
      {keepOriginal && (
        <p className="text-box-selection-note visually-hidden">
          {selection
            ? "使用原字体的文本框只能整体调整字号和颜色；取消选择后再修改。"
            : "正在使用 PDF 中的原字体。字号和颜色会保留原字体；选择其他字体可启用粗体、斜体、下划线与对齐。"}
        </p>
      )}

      <div className="text-style-font-row">
        <label className="text-box-style-field">
          <span className="visually-hidden">字体</span>
          <select
            aria-label="字体"
            value={keepOriginal ? KEEP_ORIGINAL_FONT : style.fontFamily}
            disabled={styleLocked}
            onChange={(event) => {
              const value = event.target.value;
              update({
                fontFamily:
                  value === KEEP_ORIGINAL_FONT ? target.fontFamily : value,
              });
            }}
          >
            {keepOriginal && (
              <option value={KEEP_ORIGINAL_FONT}>
                原字体 · {originalLabel}
              </option>
            )}
            <optgroup label="内置字体">
              {builtInOptions.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </optgroup>
            {wpsOptions.length > 0 && (
              <optgroup label="WPS 字体">
                {wpsOptions.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </optgroup>
            )}
            {systemOptions.length > 0 && (
              <optgroup label="本机字体">
                {systemOptions.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </optgroup>
            )}
          </select>
        </label>

        <label className="text-box-style-field">
          <span className="visually-hidden">字号</span>
          <span className="text-box-size-row">
            <input
              type="number"
              aria-label="字号"
              min={4}
              max={400}
              step={0.5}
              value={style.fontSize}
              disabled={styleLocked}
              onChange={(event) => {
                const size = Number(event.target.value);
                if (Number.isFinite(size) && size >= 4 && size <= 400) {
                  update({ fontSize: size });
                }
              }}
            />
            <select
              className="text-box-size-preset"
              aria-label="常用字号"
              value={sizePreset}
              disabled={styleLocked}
              onChange={(event) => {
                const size = Number(event.target.value);
                if (Number.isFinite(size) && size >= 4 && size <= 400) {
                  update({ fontSize: size });
                }
              }}
            >
              <option value="">常用</option>
              {FONT_SIZE_PRESETS.map((size) => (
                <option key={size} value={size}>
                  {size}
                </option>
              ))}
            </select>
          </span>
        </label>
      </div>
      <div className="text-box-style-group" role="group" aria-label="字形">
        <button
          type="button"
          aria-pressed={style.bold}
          disabled={styleLocked || keepOriginal}
          onPointerDown={keepTextSelection}
          onClick={() => update({ bold: !style.bold })}
        >
          <WorkspaceIcon name="bold" />
          <span className="visually-hidden">粗体</span>
        </button>
        <button
          type="button"
          aria-pressed={style.italic}
          disabled={styleLocked || keepOriginal}
          onPointerDown={keepTextSelection}
          onClick={() => update({ italic: !style.italic })}
        >
          <WorkspaceIcon name="italic" />
          <span className="visually-hidden">斜体</span>
        </button>
        <button
          type="button"
          aria-pressed={style.underline}
          disabled={styleLocked || keepOriginal}
          onPointerDown={keepTextSelection}
          onClick={() => update({ underline: !style.underline })}
        >
          <WorkspaceIcon name="underline" />
          <span className="visually-hidden">下划线</span>
        </button>
        <button
          type="button"
          aria-label="删除线"
          aria-pressed={style.strikeout ?? false}
          disabled={styleLocked || keepOriginal}
          onPointerDown={keepTextSelection}
          onClick={() => update({ strikeout: !style.strikeout })}
        >
          <WorkspaceIcon name="strikeout" />
        </button>
      </div>

      {!selection && (
        <div className="text-box-style-group" role="group" aria-label="对齐">
          {alignments.map((alignment) => (
            <button
              key={alignment.value}
              type="button"
              aria-pressed={target.alignment === alignment.value}
              disabled={styleLocked || keepOriginal}
              onClick={() => update({ alignment: alignment.value })}
            >
              <WorkspaceIcon
                name={
                  alignment.value === "left"
                    ? "alignLeft"
                    : alignment.value === "center"
                      ? "alignCenter"
                      : "alignRight"
                }
              />
              <span className="visually-hidden">{alignment.label}</span>
            </button>
          ))}
        </div>
      )}

      <div className="text-box-style-group">
        <button
          type="button"
          aria-label="URL 链接"
          title="URL 链接"
          disabled={!onLink || busy}
          onPointerDown={keepTextSelection}
          onClick={onLink}
        >
          <WorkspaceIcon name="link" />
        </button>
      </div>
      <TextColorPalette
        color={style.color}
        disabled={styleLocked}
        onChange={(color) => update({ color })}
      />
      <p className="text-box-font-note visually-hidden">
        编辑已有文字时优先保留原字体；原字体缺少的字优先使用本机安装的同名字体，仍缺少时再改用内置字体：仿宋改用朱雀仿宋，宋体等衬线字体改用思源宋体，其余中文改用思源黑体。可在“WPS
        字体”和“本机字体”分组中选择已安装的字体。
      </p>

      {showDelete && !selection && !editing && (
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
