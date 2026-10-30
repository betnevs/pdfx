import { useEffect, useState } from "react";
import { WorkspaceIcon, type WorkspaceIconName } from "./WorkspaceIcon";

/** Shared colour controls for original text and newly added paragraphs. */
export function TextColorPalette({
  color,
  disabled = false,
  onChange,
}: {
  color: string;
  disabled?: boolean;
  onChange: (color: string) => void;
}) {
  const readColors = (): string[] => {
    try {
      const value: unknown = JSON.parse(localStorage.getItem("pdfx:custom-colors:v1") ?? '["#000000"]');
      return Array.isArray(value) ? value.filter((c): c is string => typeof c === "string" && /^#[0-9a-f]{6}$/i.test(c)).slice(0, 32) : ["#000000"];
    } catch { return ["#000000"]; }
  };
  const [custom, setCustomState] = useState<string[]>(readColors);
  useEffect(() => {
    const update = () => setCustomState(readColors());
    window.addEventListener("pdfx:custom-colors", update);
    window.addEventListener("storage", update);
    return () => {window.removeEventListener("pdfx:custom-colors", update);window.removeEventListener("storage", update);};
  }, []);
  const setCustom = (update: (colors: string[]) => string[]) => {
    const next = update(custom).slice(0, 32);
    setCustomState(next);
    try { localStorage.setItem("pdfx:custom-colors:v1", JSON.stringify(next)); window.dispatchEvent(new Event("pdfx:custom-colors")); } catch { /* Keep this window's palette if storage is unavailable. */ }
  };
  return (
    <div className="text-color-palette">
      <h3>当前颜色</h3>
      <div className="color-row">
        <label
          className="color-swatch color-swatch--current"
          title="颜色"
          style={{ backgroundColor: color }}
        >
          <input
            type="color"
            aria-label="颜色"
            value={color}
            disabled={disabled}
            onChange={(e) => onChange(e.target.value)}
          />
        </label>
        <button
          type="button"
          aria-label="添加到自定义颜色"
          title="添加到自定义颜色"
          disabled={disabled}
          onClick={() =>
            setCustom((cs) => (cs.includes(color) ? cs : [...cs, color]))
          }
        >
          <WorkspaceIcon name="copy" />
        </button>
      </div>
      <h3>定制颜色</h3>
      <div className="color-row">
        {custom.map((c) => (
          <button
            key={c}
            className="color-swatch"
            style={{ backgroundColor: c }}
            aria-label={`颜色 ${c}`}
            aria-pressed={c === color}
            disabled={disabled}
            onClick={() => onChange(c)}
          />
        ))}
        <label className="add-color" title="添加新颜色">
          <WorkspaceIcon name="plus" />
          <input
            type="color"
            aria-label="添加新颜色"
            disabled={disabled}
            value={color}
            onChange={(e) => {
              onChange(e.target.value);
              setCustom((cs) =>
                cs.includes(e.target.value) ? cs : [...cs, e.target.value],
              );
            }}
          />
        </label>
        <button
          type="button"
          aria-label="删除自定义颜色"
          title="删除自定义颜色"
          disabled={disabled || !custom.includes(color)}
          onClick={() => setCustom((cs) => cs.filter((c) => c !== color))}
        >
          <WorkspaceIcon name="trash" />
        </button>
      </div>
    </div>
  );
}

export function TextStylePlaceholder() {
  const buttons: [WorkspaceIconName, string][] = [
    ["bold", "粗体"],
    ["italic", "斜体"],
    ["underline", "下划线"],
    ["strikeout", "删除线"],
    ["alignLeft", "左对齐"],
    ["alignCenter", "居中"],
    ["alignRight", "右对齐"],
    ["link", "URL 链接"],
  ];
  return (
    <div className="text-style-placeholder" aria-label="未选择文本的样式">
      <div className="text-style-font-row">
        <select aria-label="字体" disabled>
          <option>Font</option>
        </select>
        <select aria-label="字号" disabled>
          <option>12</option>
        </select>
      </div>
      <div className="text-style-buttons">
        {buttons.map(([icon, label]) => (
          <button key={icon} type="button" aria-label={label} disabled>
            <WorkspaceIcon name={icon} />
          </button>
        ))}
      </div>
      <TextColorPalette color="#000000" disabled onChange={() => {}} />
    </div>
  );
}
