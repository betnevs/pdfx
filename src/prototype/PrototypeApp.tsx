import type { CSSProperties, KeyboardEvent, MouseEvent, RefObject } from "react";
import { useEffect, useRef, useState } from "react";

import { Icon, type IconName } from "./Icon";
import {
  DOCUMENT_ANNOTATIONS,
  PAGE_COUNT,
  getPage,
  type PlaceholderAnnotation,
  type PlaceholderPage,
  type PlaceholderShape,
} from "./content";
import { copy } from "./copy";
import {
  ANNOTATION_COLORS,
  DEFAULT_TOOL,
  FALLBACK_VIEWPORT,
  FONT_STACKS,
  MODE_TAB,
  NO_SELECTION,
  PAGE_HEIGHT,
  PAGE_WIDTH,
  SHAPE_FILL_COLORS,
  SHAPE_STROKE_COLORS,
  TAB_MODE,
  TAB_ORDER,
  TAB_TOOLS,
  TEXT_COLORS,
  TEXT_FONT_IDS,
  ZOOM_STEP,
  clampPage,
  clampZoom,
  fitPageZoom,
  fitWidthZoom,
  parseZoom,
  type AnnotationStyle,
  type ColorOption,
  type Selection,
  type ShapeStyle,
  type TabId,
  type TextAlign,
  type TextFontId,
  type TextStyle,
  type ToolId,
  type ViewportSize,
  type WorkspaceMode,
  type ZoomMode,
} from "./model";

type TextSelection = Extract<Selection, { kind: "text" }>;

const KIND_ICON: Record<ToolId, IconName> = {  highlight: "highlight",
  underline: "underline",
  strikeout: "strikeout",
  note: "note",
  ink: "ink",
  rect: "rect",
  ellipse: "ellipse",
  line: "line",
  arrow: "arrow",
  image: "image",
  signature: "signature",
  link: "link",
  stamp: "stamp",
  editText: "editText",
  addText: "addText",
  formText: "formText",
  formCheckbox: "formCheckbox",
  formRadio: "formRadio",
  formSelect: "formSelect",
};

const LEAD_STYLE: TextStyle = {
  fontId: "serif",
  fontSize: 17,
  color: "#23262d",
  bold: false,
  italic: false,
  underline: false,
  align: "left",
};

const BULLET_STYLE: TextStyle = {
  fontId: "serif",
  fontSize: 15,
  color: "#23262d",
  bold: false,
  italic: false,
  underline: false,
  align: "left",
};

function useCanvasViewport(ref: RefObject<HTMLElement | null>): ViewportSize {
  const [size, setSize] = useState<ViewportSize>(FALLBACK_VIEWPORT);

  useEffect(() => {
    const element = ref.current;
    if (!element) {
      return;
    }
    const measure = () => {
      if (element.clientWidth > 0 && element.clientHeight > 0) {
        setSize({ width: element.clientWidth, height: element.clientHeight });
      }
    };
    measure();
    if (typeof ResizeObserver === "undefined") {
      return;
    }
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref]);

  return size;
}

function rectStyle(rect: PlaceholderAnnotation["rect"]): CSSProperties {
  return {
    left: `${rect.left}%`,
    top: `${rect.top}%`,
    width: `${rect.width}%`,
    height: `${rect.height}%`,
  };
}

function withAlpha(hex: string, alpha: number): string {
  if (!hex.startsWith("#") || hex.length !== 7) {
    return hex;
  }
  const value = Math.round(alpha * 255)
    .toString(16)
    .padStart(2, "0");
  return `${hex}${value}`;
}

function textSnippet(text: string): string {
  return text.length > 18 ? `${text.slice(0, 18)}…` : text;
}

function textLabel(text: string): string {
  return `${copy.kind.text}：${textSnippet(text)}`;
}

function textStyleToCss(style: TextStyle): CSSProperties {
  return {
    fontFamily: FONT_STACKS[style.fontId],
    fontSize: `${style.fontSize}px`,
    fontWeight: style.bold ? 700 : 400,
    fontStyle: style.italic ? "italic" : "normal",
    textDecoration: style.underline ? "underline" : "none",
    color: style.color,
    textAlign: style.align,
  };
}

function defaultAnnotationStyle(
  annotation: PlaceholderAnnotation,
): AnnotationStyle {
  return {
    color: annotation.color,
    opacity: 100,
    note: annotation.note ?? "",
  };
}

function defaultShapeStyle(shape: PlaceholderShape): ShapeStyle {
  return {
    fill: shape.fill,
    stroke: shape.stroke,
    strokeWidth: shape.strokeWidth,
    opacity: 100,
  };
}

function AnnotationView({
  annotation,
  visualStyle,
  selected,
  onSelect,
}: {
  annotation: PlaceholderAnnotation;
  visualStyle: AnnotationStyle;
  selected: boolean;
  onSelect: () => void;
}) {
  const className = `page-annotation page-annotation--${annotation.kind}`;
  const shared = {
    type: "button" as const,
    className,
    "aria-label": annotation.label,
    "aria-pressed": selected,
    "data-selected": selected || undefined,
    onClick: (event: MouseEvent<HTMLButtonElement>) => {
      event.stopPropagation();
      onSelect();
    },
  };
  const opacity = visualStyle.opacity / 100;

  if (annotation.kind === "note") {
    return (
      <button
        {...shared}
        style={{
          ...rectStyle(annotation.rect),
          backgroundColor: visualStyle.color,
          opacity,
        }}
      >
        <Icon name="note" />
      </button>
    );
  }
  if (annotation.kind === "underline") {
    return (
      <button
        {...shared}
        style={{
          ...rectStyle(annotation.rect),
          borderBottomColor: visualStyle.color,
          opacity,
        }}
      />
    );
  }
  if (annotation.kind === "strikeout") {
    return (
      <button {...shared} style={{ ...rectStyle(annotation.rect), opacity }}>
        <span
          className="annotation-strike"
          style={{ backgroundColor: visualStyle.color }}
        />
      </button>
    );
  }
  return (
    <button
      {...shared}
      style={{
        ...rectStyle(annotation.rect),
        backgroundColor: withAlpha(visualStyle.color, opacity * 0.42),
      }}
    />
  );
}

function ShapeView({
  shape,
  visualStyle,
  selected,
  onSelect,
}: {
  shape: PlaceholderShape;
  visualStyle: ShapeStyle;
  selected: boolean;
  onSelect: () => void;
}) {
  const shared = {
    type: "button" as const,
    className: `page-shape page-shape--${shape.kind}`,
    "aria-label": shape.label,
    "aria-pressed": selected,
    "data-selected": selected || undefined,
    onClick: (event: MouseEvent<HTMLButtonElement>) => {
      event.stopPropagation();
      onSelect();
    },
  };
  const opacity = visualStyle.opacity / 100;

  if (shape.kind === "line" || shape.kind === "arrow") {
    return (
      <button
        {...shared}
        style={{ ...rectStyle(shape.rect), opacity }}
      >
        <svg
          className="shape-svg"
          viewBox="0 0 100 100"
          preserveAspectRatio="none"
          aria-hidden="true"
        >
          <line
            x1="2"
            y1="98"
            x2="98"
            y2="2"
            stroke={visualStyle.stroke}
            strokeWidth={visualStyle.strokeWidth}
            vectorEffect="non-scaling-stroke"
          />
          {shape.kind === "arrow" ? (
            <polyline
              points="78,2 98,2 98,22"
              fill="none"
              stroke={visualStyle.stroke}
              strokeWidth={visualStyle.strokeWidth}
              vectorEffect="non-scaling-stroke"
            />
          ) : null}
        </svg>
      </button>
    );
  }
  return (
    <button
      {...shared}
      style={{
        ...rectStyle(shape.rect),
        borderColor: visualStyle.stroke,
        borderWidth: visualStyle.strokeWidth,
        backgroundColor:
          visualStyle.fill === "none" ? "transparent" : visualStyle.fill,
        opacity,
      }}
    />
  );
}

function PageContent({
  page,
  selection,
  textStyles,
  annotationStyles,
  shapeStyles,
  onSelectText,
  onSelectAnnotation,
  onSelectShape,
}: {
  page: PlaceholderPage;
  selection: Selection;
  textStyles: Record<string, TextStyle>;
  annotationStyles: Record<string, AnnotationStyle>;
  shapeStyles: Record<string, ShapeStyle>;
  onSelectText: (selection: TextSelection, defaults: TextStyle) => void;
  onSelectAnnotation: (page: number, annotation: PlaceholderAnnotation) => void;
  onSelectShape: (page: number, shape: PlaceholderShape) => void;
}) {
  const renderText = (
    id: string,
    text: string,
    className: string,
    defaults: TextStyle,
  ) => {
    const visualStyle = textStyles[id] ?? defaults;
    const selected = selection.kind === "text" && selection.id === id;
    return (
      <button
        key={id}
        type="button"
        className={`page-text ${className}`}
        aria-label={textLabel(text)}
        aria-pressed={selected}
        data-selected={selected || undefined}
        style={textStyleToCss(visualStyle)}
        onClick={(event) => {
          event.stopPropagation();
          onSelectText(
            { kind: "text", id, label: textSnippet(text), page: page.id },
            defaults,
          );
        }}
      >
        {text}
      </button>
    );
  };

  return (
    <div className="page-body">
      <p className="page-eyebrow">{page.eyebrow}</p>
      <h2 className="page-title">{page.title}</h2>
      {renderText(`${page.id}-lead`, page.lead, "page-lead", LEAD_STYLE)}
      <h3 className="page-heading">{page.heading}</h3>
      <ul className="page-bullets">
        {page.bullets.map((bullet, index) => (
          <li key={bullet}>
            {renderText(
              `${page.id}-bullet-${index}`,
              bullet,
              "page-bullet",
              BULLET_STYLE,
            )}
          </li>
        ))}
      </ul>
      {renderText(`${page.id}-body`, page.body, "page-paragraph", BULLET_STYLE)}
      <p className="page-footer">{page.footer}</p>
      {page.annotations.map((annotation) => (
        <AnnotationView
          key={annotation.id}
          annotation={annotation}
          visualStyle={
            annotationStyles[annotation.id] ?? defaultAnnotationStyle(annotation)
          }
          selected={
            selection.kind === "annotation" && selection.id === annotation.id
          }
          onSelect={() => onSelectAnnotation(page.id, annotation)}
        />
      ))}
      {page.shapes.map((shape) => (
        <ShapeView
          key={shape.id}
          shape={shape}
          visualStyle={shapeStyles[shape.id] ?? defaultShapeStyle(shape)}
          selected={selection.kind === "shape" && selection.id === shape.id}
          onSelect={() => onSelectShape(page.id, shape)}
        />
      ))}
    </div>
  );
}

function ThumbnailRail({
  currentPage,
  onSelectPage,
  onCollapse,
}: {
  currentPage: number;
  onSelectPage: (page: number) => void;
  onCollapse: () => void;
}) {
  return (
    <nav className="thumbnail-rail" aria-label={copy.thumbnails.title}>
      <div className="rail-heading">
        <span className="rail-title">{copy.thumbnails.title}</span>
        <button
          type="button"
          className="mini-icon-button"
          aria-label={copy.thumbnails.collapse}
          aria-expanded={true}
          onClick={onCollapse}
        >
          <Icon name="collapseLeft" />
        </button>
      </div>
      <div className="thumbnail-list">
        {Array.from({ length: PAGE_COUNT }, (_, index) => index + 1).map((pageNumber) => {
          const page = getPage(pageNumber);
          const active = pageNumber === currentPage;
          return (
            <button
              key={pageNumber}
              type="button"
              className="thumbnail"
              aria-label={copy.thumbnails.page(pageNumber)}
              aria-current={active ? "page" : undefined}
              onClick={() => onSelectPage(pageNumber)}
            >
              <span className="thumbnail-paper" aria-hidden="true">
                <span className="thumb-title">{page.title}</span>
                <span className="thumb-line thumb-line--wide" />
                <span className="thumb-line" />
                <span className="thumb-line thumb-line--short" />
                <span className="thumb-blocks">
                  <span />
                  <span />
                  <span />
                </span>
              </span>
              <span className="thumbnail-number">{pageNumber}</span>
            </button>
          );
        })}
      </div>
    </nav>
  );
}

function Toolbar({
  activeTab,
  activeTool,
  onSelectMode,
  onSelectTab,
  onSelectTool,
}: {
  activeTab: TabId;
  activeTool: ToolId;
  onSelectMode: (mode: WorkspaceMode) => void;
  onSelectTab: (tab: TabId) => void;
  onSelectTool: (tool: ToolId) => void;
}) {
  const mode: WorkspaceMode = TAB_MODE[activeTab];

  const handleTabKeyDown = (
    event: KeyboardEvent<HTMLButtonElement>,
    tab: TabId,
  ) => {
    const index = TAB_ORDER.indexOf(tab);
    let nextIndex: number | null = null;
    if (event.key === "ArrowRight") {
      nextIndex = (index + 1) % TAB_ORDER.length;
    } else if (event.key === "ArrowLeft") {
      nextIndex = (index - 1 + TAB_ORDER.length) % TAB_ORDER.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = TAB_ORDER.length - 1;
    }
    if (nextIndex === null) {
      return;
    }
    event.preventDefault();
    onSelectTab(TAB_ORDER[nextIndex]);
    const tablist = event.currentTarget.closest('[role="tablist"]');
    const buttons =
      tablist?.querySelectorAll<HTMLButtonElement>('[role="tab"]');
    buttons?.[nextIndex]?.focus();
  };

  return (
    <div className="proto-toolbar">
      <div className="mode-switch" role="group" aria-label={copy.modes.groupLabel}>
        <button
          type="button"
          className="mode-button"
          aria-pressed={mode === "annotate"}
          onClick={() => onSelectMode("annotate")}
        >
          {copy.modes.annotate}
        </button>
        <button
          type="button"
          className="mode-button"
          aria-pressed={mode === "edit"}
          onClick={() => onSelectMode("edit")}
        >
          {copy.modes.edit}
        </button>
      </div>
      <div className="tabbar" role="tablist" aria-label={copy.tabs.listLabel}>
        {TAB_ORDER.map((tab) => (
          <button
            key={tab}
            type="button"
            role="tab"
            className="tab"
            aria-selected={tab === activeTab}
            tabIndex={tab === activeTab ? 0 : -1}
            onClick={() => onSelectTab(tab)}
            onKeyDown={(event) => handleTabKeyDown(event, tab)}
          >
            {copy.tabs[tab]}
          </button>
        ))}
      </div>
      <div
        className="tool-strip"
        role="group"
        aria-label={copy.toolbar.toolGroup(copy.tabs[activeTab])}
      >
        {TAB_TOOLS[activeTab].map((tool) => (
          <button
            key={tool}
            type="button"
            className="tool-button"
            aria-pressed={tool === activeTool}
            onClick={() => onSelectTool(tool)}
          >
            <Icon name={KIND_ICON[tool]} />
            <span>{copy.tools[tool].label}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

function NumberField({
  label,
  value,
  min,
  max,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  onChange: (value: number) => void;
}) {
  const [draft, setDraft] = useState(String(value));

  useEffect(() => {
    setDraft(String(value));
  }, [value]);

  const commit = (raw: string) => {
    const parsed = Number.parseFloat(raw);
    if (!Number.isFinite(parsed)) {
      setDraft(String(value));
      return;
    }
    const clamped = Math.min(max, Math.max(min, Math.round(parsed)));
    setDraft(String(clamped));
    onChange(clamped);
  };

  return (
    <input
      type="number"
      className="property-input"
      aria-label={label}
      value={draft}
      min={min}
      max={max}
      onChange={(event) => {
        const raw = event.target.value;
        setDraft(raw);
        const parsed = Number.parseFloat(raw);
        if (Number.isFinite(parsed) && parsed >= 0) {
          onChange(parsed);
        }
      }}
      onBlur={(event) => commit(event.target.value)}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          commit((event.target as HTMLInputElement).value);
        }
      }}
    />
  );
}

function ColorSwatches({
  label,
  colors,
  value,
  onSelect,
}: {
  label: string;
  colors: readonly ColorOption[];
  value: string;
  onSelect: (value: string) => void;
}) {
  return (
    <div className="field">
      <span className="field-label">{label}</span>
      <div className="swatch-row">
        {colors.map((color) => (
          <button
            key={color.id}
            type="button"
            className={
              color.value === "none" ? "swatch swatch--none" : "swatch"
            }
            style={
              color.value === "none"
                ? undefined
                : { backgroundColor: color.value }
            }
            aria-label={`${label}：${copy.colors[color.id]}`}
            aria-pressed={value === color.value}
            onClick={() => onSelect(color.value)}
          />
        ))}
      </div>
    </div>
  );
}

function RangeField({
  label,
  value,
  onChange,
}: {
  label: string;
  value: number;
  onChange: (value: number) => void;
}) {
  return (
    <div className="field">
      <span className="field-label">{label}</span>
      <div className="range-row">
        <input
          type="range"
          className="property-range"
          aria-label={label}
          min={10}
          max={100}
          step={5}
          value={value}
          onChange={(event) => onChange(Number(event.target.value))}
        />
        <span className="range-value">{value}%</span>
      </div>
    </div>
  );
}

function TextProperties({
  label,
  style,
  onChange,
}: {
  label: string;
  style: TextStyle;
  onChange: (patch: Partial<TextStyle>) => void;
}) {
  const alignOptions: readonly {
    value: TextAlign;
    label: string;
    icon: IconName;
  }[] = [
    { value: "left", label: copy.properties.text.alignLeft, icon: "alignLeft" },
    {
      value: "center",
      label: copy.properties.text.alignCenter,
      icon: "alignCenter",
    },
    { value: "right", label: copy.properties.text.alignRight, icon: "alignRight" },
  ];

  return (
    <>
      <section className="panel-section">
        <h2 className="panel-section-title">{copy.properties.text.title}</h2>
        <p className="panel-description">
          {copy.properties.text.selected(label)}
        </p>
      </section>
      <section className="panel-section">
        <div className="field">
          <span className="field-label">{copy.properties.text.font}</span>
          <select
            className="property-select"
            aria-label={copy.properties.text.font}
            value={style.fontId}
            onChange={(event) =>
              onChange({ fontId: event.target.value as TextFontId })
            }
          >
            {TEXT_FONT_IDS.map((fontId) => (
              <option key={fontId} value={fontId}>
                {copy.fonts[fontId]}
              </option>
            ))}
          </select>
        </div>
        <div className="field">
          <span className="field-label">{copy.properties.text.fontSize}</span>
          <NumberField
            label={copy.properties.text.fontSize}
            value={style.fontSize}
            min={6}
            max={144}
            onChange={(fontSize) => onChange({ fontSize })}
          />
        </div>
        <ColorSwatches
          label={copy.properties.text.color}
          colors={TEXT_COLORS}
          value={style.color}
          onSelect={(color) => onChange({ color })}
        />
        <div className="field">
          <span className="field-label">{copy.properties.text.style}</span>
          <div
            className="segmented"
            role="group"
            aria-label={copy.properties.text.style}
          >
            <button
              type="button"
              className="segment-button"
              aria-label={copy.properties.text.bold}
              aria-pressed={style.bold}
              onClick={() => onChange({ bold: !style.bold })}
            >
              <Icon name="bold" />
            </button>
            <button
              type="button"
              className="segment-button"
              aria-label={copy.properties.text.italic}
              aria-pressed={style.italic}
              onClick={() => onChange({ italic: !style.italic })}
            >
              <Icon name="italic" />
            </button>
            <button
              type="button"
              className="segment-button"
              aria-label={copy.properties.text.underline}
              aria-pressed={style.underline}
              onClick={() => onChange({ underline: !style.underline })}
            >
              <Icon name="underline" />
            </button>
          </div>
        </div>
        <div className="field">
          <span className="field-label">{copy.properties.text.align}</span>
          <div
            className="segmented"
            role="group"
            aria-label={copy.properties.text.align}
          >
            {alignOptions.map((option) => (
              <button
                key={option.value}
                type="button"
                className="segment-button"
                aria-label={option.label}
                aria-pressed={style.align === option.value}
                onClick={() => onChange({ align: option.value })}
              >
                <Icon name={option.icon} />
              </button>
            ))}
          </div>
        </div>
      </section>
    </>
  );
}

function AnnotationProperties({
  label,
  style,
  onChange,
}: {
  label: string;
  style: AnnotationStyle;
  onChange: (patch: Partial<AnnotationStyle>) => void;
}) {
  return (
    <>
      <section className="panel-section">
        <h2 className="panel-section-title">
          {copy.properties.annotation.title}
        </h2>
        <p className="panel-description">
          {copy.properties.annotation.selected(label)}
        </p>
      </section>
      <section className="panel-section">
        <ColorSwatches
          label={copy.properties.annotation.color}
          colors={ANNOTATION_COLORS}
          value={style.color}
          onSelect={(color) => onChange({ color })}
        />
        <RangeField
          label={copy.properties.annotation.opacity}
          value={style.opacity}
          onChange={(opacity) => onChange({ opacity })}
        />
        <div className="field">
          <span className="field-label">
            {copy.properties.annotation.content}
          </span>
          <textarea
            className="property-textarea"
            aria-label={copy.properties.annotation.content}
            placeholder={copy.properties.annotation.contentPlaceholder}
            value={style.note}
            onChange={(event) => onChange({ note: event.target.value })}
          />
        </div>
        <div className="field">
          <span className="field-label">
            {copy.properties.annotation.author}
          </span>
          <span className="property-value">
            {copy.properties.annotation.authorValue}
          </span>
        </div>
      </section>
    </>
  );
}

function ShapeProperties({
  label,
  style,
  onChange,
}: {
  label: string;
  style: ShapeStyle;
  onChange: (patch: Partial<ShapeStyle>) => void;
}) {
  return (
    <>
      <section className="panel-section">
        <h2 className="panel-section-title">{copy.properties.shape.title}</h2>
        <p className="panel-description">
          {copy.properties.shape.selected(label)}
        </p>
      </section>
      <section className="panel-section">
        <ColorSwatches
          label={copy.properties.shape.fill}
          colors={SHAPE_FILL_COLORS}
          value={style.fill}
          onSelect={(fill) => onChange({ fill })}
        />
        <ColorSwatches
          label={copy.properties.shape.stroke}
          colors={SHAPE_STROKE_COLORS}
          value={style.stroke}
          onSelect={(stroke) => onChange({ stroke })}
        />
        <div className="field">
          <span className="field-label">{copy.properties.shape.strokeWidth}</span>
          <NumberField
            label={copy.properties.shape.strokeWidth}
            value={style.strokeWidth}
            min={1}
            max={24}
            onChange={(strokeWidth) => onChange({ strokeWidth })}
          />
        </div>
        <RangeField
          label={copy.properties.shape.opacity}
          value={style.opacity}
          onChange={(opacity) => onChange({ opacity })}
        />
      </section>
    </>
  );
}

function TabPanel({
  activeTab,
  onLocateAnnotation,
}: {
  activeTab: TabId;
  onLocateAnnotation: (page: number, annotation: PlaceholderAnnotation) => void;
}) {
  const panel = copy.properties.panels[activeTab];
  return (
    <>
      <section className="panel-section">
        <h2 className="panel-section-title">{panel.title}</h2>
        <p className="panel-description">{panel.description}</p>
      </section>
      {activeTab === "annotate" ? (
        <section className="panel-section">
          <h3 className="field-label">{copy.properties.panels.annotate.list}</h3>
          <ul className="annotation-list">
            {DOCUMENT_ANNOTATIONS.map(({ page, annotation }) => (
              <li key={annotation.id}>
                <button
                  type="button"
                  className="annotation-list-item"
                  onClick={() => onLocateAnnotation(page, annotation)}
                >
                  <span>{annotation.label}</span>
                  <span className="annotation-page">
                    {copy.thumbnails.page(page)}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      {"comingSoon" in panel ? (
        <p className="coming-soon">{panel.comingSoon}</p>
      ) : null}
    </>
  );
}

function PropertiesPanel({
  activeTab,
  selection,
  textStyle,
  annotationStyle,
  shapeStyle,
  onTextChange,
  onAnnotationChange,
  onShapeChange,
  onLocateAnnotation,
  onCollapse,
}: {
  activeTab: TabId;
  selection: Selection;
  textStyle?: TextStyle;
  annotationStyle?: AnnotationStyle;
  shapeStyle?: ShapeStyle;
  onTextChange: (patch: Partial<TextStyle>) => void;
  onAnnotationChange: (patch: Partial<AnnotationStyle>) => void;
  onShapeChange: (patch: Partial<ShapeStyle>) => void;
  onLocateAnnotation: (page: number, annotation: PlaceholderAnnotation) => void;
  onCollapse: () => void;
}) {
  return (
    <aside className="properties-panel" aria-label={copy.properties.title}>
      <div className="panel-heading">
        <span className="panel-title">{copy.properties.title}</span>
        <button
          type="button"
          className="mini-icon-button"
          aria-label={copy.properties.collapse}
          aria-expanded={true}
          onClick={onCollapse}
        >
          <Icon name="collapseRight" />
        </button>
      </div>
      <div className="panel-content">
        {selection.kind === "text" && textStyle ? (
          <TextProperties
            label={selection.label}
            style={textStyle}
            onChange={onTextChange}
          />
        ) : null}
        {selection.kind === "annotation" && annotationStyle ? (
          <AnnotationProperties
            label={selection.label}
            style={annotationStyle}
            onChange={onAnnotationChange}
          />
        ) : null}
        {selection.kind === "shape" && shapeStyle ? (
          <ShapeProperties
            label={selection.label}
            style={shapeStyle}
            onChange={onShapeChange}
          />
        ) : null}
        {selection.kind === "none" ? (
          <TabPanel
            activeTab={activeTab}
            onLocateAnnotation={onLocateAnnotation}
          />
        ) : null}
      </div>
    </aside>
  );
}

function StatusBar({
  currentPage,
  zoom,
  zoomMode,
  onSelectPage,
  onZoomChange,
  onZoomIn,
  onZoomOut,
  onFitWidth,
  onFitPage,
}: {
  currentPage: number;
  zoom: number;
  zoomMode: ZoomMode;
  onSelectPage: (page: number) => void;
  onZoomChange: (zoom: number) => void;
  onZoomIn: () => void;
  onZoomOut: () => void;
  onFitWidth: () => void;
  onFitPage: () => void;
}) {
  const [draft, setDraft] = useState(String(currentPage));
  const [zoomDraft, setZoomDraft] = useState(`${zoom}%`);

  useEffect(() => {
    setDraft(String(currentPage));
  }, [currentPage]);

  useEffect(() => {
    setZoomDraft(`${zoom}%`);
  }, [zoom]);

  const commitDraft = () => {
    const next = clampPage(Number(draft), PAGE_COUNT);
    setDraft(String(next));
    onSelectPage(next);
  };

  const commitZoomDraft = () => {
    const next = parseZoom(zoomDraft);
    setZoomDraft(`${next}%`);
    onZoomChange(next);
  };

  return (
    <footer className="status-bar">
      <div className="page-nav">
        <button
          type="button"
          className="mini-icon-button"
          aria-label={copy.bottom.prev}
          disabled={currentPage <= 1}
          onClick={() => onSelectPage(currentPage - 1)}
        >
          <Icon name="chevronLeft" />
        </button>
        <input
          type="number"
          className="page-input"
          aria-label={copy.bottom.pageLabel}
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onBlur={commitDraft}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              commitDraft();
            }
          }}
        />
        <span className="page-total">{copy.bottom.total(PAGE_COUNT)}</span>
        <button
          type="button"
          className="mini-icon-button"
          aria-label={copy.bottom.next}
          disabled={currentPage >= PAGE_COUNT}
          onClick={() => onSelectPage(currentPage + 1)}
        >
          <Icon name="chevronRight" />
        </button>
      </div>
      <div className="zoom-controls">
        <button
          type="button"
          className="mini-icon-button"
          aria-label={copy.bottom.zoomOut}
          onClick={onZoomOut}
        >
          <Icon name="zoomOut" />
        </button>
        <input
          type="text"
          className="zoom-input"
          aria-label={copy.bottom.zoomLabel}
          value={zoomDraft}
          onChange={(event) => setZoomDraft(event.target.value)}
          onBlur={commitZoomDraft}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              commitZoomDraft();
            }
          }}
        />
        <button
          type="button"
          className="mini-icon-button"
          aria-label={copy.bottom.zoomIn}
          onClick={onZoomIn}
        >
          <Icon name="zoomIn" />
        </button>
        <span className="zoom-divider" aria-hidden="true" />
        <button
          type="button"
          className="fit-button"
          aria-pressed={zoomMode === "fit-width"}
          onClick={onFitWidth}
        >
          <Icon name="fitWidth" />
          {copy.bottom.fitWidth}
        </button>
        <button
          type="button"
          className="fit-button"
          aria-pressed={zoomMode === "fit-page"}
          onClick={onFitPage}
        >
          <Icon name="fitPage" />
          {copy.bottom.fitPage}
        </button>
      </div>
    </footer>
  );
}

export function PrototypeApp() {
  const [activeTab, setActiveTab] = useState<TabId>("annotate");
  const [activeTool, setActiveTool] = useState<ToolId>(DEFAULT_TOOL.annotate);
  const [currentPage, setCurrentPage] = useState(1);
  const [zoom, setZoom] = useState(100);
  const [zoomMode, setZoomMode] = useState<ZoomMode>("custom");
  const [selection, setSelection] = useState<Selection>(NO_SELECTION);
  const [textStyles, setTextStyles] = useState<Record<string, TextStyle>>({});
  const [annotationStyles, setAnnotationStyles] = useState<
    Record<string, AnnotationStyle>
  >({});
  const [shapeStyles, setShapeStyles] = useState<Record<string, ShapeStyle>>({});
  const [thumbsCollapsed, setThumbsCollapsed] = useState(false);
  const [propertiesCollapsed, setPropertiesCollapsed] = useState(false);
  const canvasRef = useRef<HTMLElement>(null);
  const viewport = useCanvasViewport(canvasRef);
  const fitWidth = fitWidthZoom(viewport);
  const fitPage = fitPageZoom(viewport);
  const effectiveZoom =
    zoomMode === "fit-width"
      ? fitWidth
      : zoomMode === "fit-page"
        ? fitPage
        : zoom;
  const page = getPage(currentPage);
  const tool = copy.tools[activeTool];
  const scale = effectiveZoom / 100;

  const clearSelection = () => setSelection(NO_SELECTION);

  const selectTab = (tab: TabId) => {
    setActiveTab(tab);
    setActiveTool(DEFAULT_TOOL[tab]);
    clearSelection();
  };

  const selectMode = (mode: WorkspaceMode) => {
    selectTab(MODE_TAB[mode]);
  };

  const selectPage = (pageNumber: number) => {
    setCurrentPage(clampPage(pageNumber, PAGE_COUNT));
    clearSelection();
  };

  const setCustomZoom = (next: number) => {
    setZoomMode("custom");
    setZoom(clampZoom(next));
  };

  const selectText = (next: TextSelection, defaults: TextStyle) => {
    setSelection(next);
    setTextStyles((previous) =>
      previous[next.id] ? previous : { ...previous, [next.id]: defaults },
    );
  };

  const selectAnnotation = (pageNumber: number, annotation: PlaceholderAnnotation) => {
    setSelection({
      kind: "annotation",
      id: annotation.id,
      label: annotation.label,
      page: pageNumber,
    });
    setAnnotationStyles((previous) =>
      previous[annotation.id]
        ? previous
        : {
            ...previous,
            [annotation.id]: defaultAnnotationStyle(annotation),
          },
    );
  };

  const selectShape = (pageNumber: number, shape: PlaceholderShape) => {
    setSelection({
      kind: "shape",
      id: shape.id,
      label: shape.label,
      page: pageNumber,
    });
    setShapeStyles((previous) =>
      previous[shape.id]
        ? previous
        : { ...previous, [shape.id]: defaultShapeStyle(shape) },
    );
  };

  const locateAnnotation = (pageNumber: number, annotation: PlaceholderAnnotation) => {
    setCurrentPage(clampPage(pageNumber, PAGE_COUNT));
    selectAnnotation(pageNumber, annotation);
  };

  const changeText = (patch: Partial<TextStyle>) => {
    if (selection.kind !== "text") {
      return;
    }
    const id = selection.id;
    setTextStyles((previous) => {
      const current = previous[id];
      return current ? { ...previous, [id]: { ...current, ...patch } } : previous;
    });
  };

  const changeAnnotation = (patch: Partial<AnnotationStyle>) => {
    if (selection.kind !== "annotation") {
      return;
    }
    const id = selection.id;
    setAnnotationStyles((previous) => {
      const current = previous[id];
      return current ? { ...previous, [id]: { ...current, ...patch } } : previous;
    });
  };

  const changeShape = (patch: Partial<ShapeStyle>) => {
    if (selection.kind !== "shape") {
      return;
    }
    const id = selection.id;
    setShapeStyles((previous) => {
      const current = previous[id];
      return current ? { ...previous, [id]: { ...current, ...patch } } : previous;
    });
  };

  const selectedTextStyle =
    selection.kind === "text" ? textStyles[selection.id] : undefined;
  const selectedAnnotationStyle =
    selection.kind === "annotation"
      ? annotationStyles[selection.id]
      : undefined;
  const selectedShapeStyle =
    selection.kind === "shape" ? shapeStyles[selection.id] : undefined;

  return (
    <div
      className="prototype-shell"
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          clearSelection();
        }
      }}
    >
      <header className="proto-header">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true">
            P
          </span>
          <h1>{copy.app.name}</h1>
        </div>
        <div className="document-chip">
          <Icon name="page" />
          <span className="document-name">{copy.app.documentName}</span>
          <span className="unsaved-dot" aria-hidden="true" />
          <span className="unsaved-label">{copy.app.unsaved}</span>
        </div>
        <span className="prototype-badge">{copy.app.prototypeBadge}</span>
        <div className="header-actions">
          <button
            type="button"
            className="mini-icon-button"
            aria-label={copy.header.undo}
            disabled
          >
            <Icon name="undo" />
          </button>
          <button
            type="button"
            className="mini-icon-button"
            aria-label={copy.header.redo}
            disabled
          >
            <Icon name="redo" />
          </button>
          <button
            type="button"
            className="save-button"
            title={copy.header.saveHint}
            disabled
          >
            <Icon name="save" />
            {copy.header.save}
          </button>
        </div>
      </header>

      <Toolbar
        activeTab={activeTab}
        activeTool={activeTool}
        onSelectMode={selectMode}
        onSelectTab={selectTab}
        onSelectTool={setActiveTool}
      />

      <div
        className="proto-workspace"
        style={{
          gridTemplateColumns: `${thumbsCollapsed ? 48 : 176}px minmax(0, 1fr) ${
            propertiesCollapsed ? 48 : 288
          }px`,
        }}
      >
        {thumbsCollapsed ? (
          <div className="collapsed-panel collapsed-panel--left">
            <button
              type="button"
              className="mini-icon-button"
              aria-label={copy.thumbnails.expand}
              aria-expanded={false}
              onClick={() => setThumbsCollapsed(false)}
            >
              <Icon name="expandLeft" />
            </button>
          </div>
        ) : (
          <ThumbnailRail
            currentPage={currentPage}
            onSelectPage={selectPage}
            onCollapse={() => setThumbsCollapsed(true)}
          />
        )}
        <main
          ref={canvasRef}
          className="canvas-region"
          aria-label={copy.canvas.title}
          onClick={clearSelection}
        >
          <div className="canvas-tool-hint" role="status">
            <span className="canvas-tool-name">
              {copy.toolbar.activeTool(tool.label)}
            </span>
            <span className="canvas-tool-hint-text">{tool.hint}</span>
          </div>
          <div className="canvas-scroll">
            <div
              className="page-frame"
              style={{ width: PAGE_WIDTH * scale, height: PAGE_HEIGHT * scale }}
            >
              <div
                className="pdf-page"
                style={{ transform: `scale(${scale})` }}
                data-page={currentPage}
              >
                <PageContent
                  page={page}
                  selection={selection}
                  textStyles={textStyles}
                  annotationStyles={annotationStyles}
                  shapeStyles={shapeStyles}
                  onSelectText={selectText}
                  onSelectAnnotation={selectAnnotation}
                  onSelectShape={selectShape}
                />
              </div>
            </div>
            <p className="canvas-placeholder-note">{copy.canvas.placeholderNote}</p>
          </div>
        </main>
        {propertiesCollapsed ? (
          <div className="collapsed-panel collapsed-panel--right">
            <button
              type="button"
              className="mini-icon-button"
              aria-label={copy.properties.expand}
              aria-expanded={false}
              onClick={() => setPropertiesCollapsed(false)}
            >
              <Icon name="expandRight" />
            </button>
          </div>
        ) : (
          <PropertiesPanel
            activeTab={activeTab}
            selection={selection}
            textStyle={selectedTextStyle}
            annotationStyle={selectedAnnotationStyle}
            shapeStyle={selectedShapeStyle}
            onTextChange={changeText}
            onAnnotationChange={changeAnnotation}
            onShapeChange={changeShape}
            onLocateAnnotation={locateAnnotation}
            onCollapse={() => setPropertiesCollapsed(true)}
          />
        )}
      </div>

      <StatusBar
        currentPage={currentPage}
        zoom={effectiveZoom}
        zoomMode={zoomMode}
        onSelectPage={selectPage}
        onZoomChange={setCustomZoom}
        onZoomIn={() => setCustomZoom(effectiveZoom + ZOOM_STEP)}
        onZoomOut={() => setCustomZoom(effectiveZoom - ZOOM_STEP)}
        onFitWidth={() => setZoomMode("fit-width")}
        onFitPage={() => setZoomMode("fit-page")}
      />
    </div>
  );
}
