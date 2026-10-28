/**
 * Placeholder document content for the M0 prototype.
 *
 * This deliberately looks like a real PDF so the layout can be reviewed, but it
 * is plain React markup: no PDF engine, no IPC. Copy lives here (rather than in
 * components) so the placeholder document can also be localised later.
 */

export type AnnotationKind = "highlight" | "underline" | "strikeout" | "note";

export type ShapeKind = "rect" | "ellipse" | "line" | "arrow";

/** Position as percentages of the page box. */
export type Rect = {
  left: number;
  top: number;
  width: number;
  height: number;
};

export type PlaceholderAnnotation = {
  id: string;
  kind: AnnotationKind;
  label: string;
  color: string;
  rect: Rect;
  note?: string;
};

export type PlaceholderShape = {
  id: string;
  kind: ShapeKind;
  label: string;
  fill: string;
  stroke: string;
  strokeWidth: number;
  rect: Rect;
};

export type PlaceholderPage = {
  id: number;
  eyebrow: string;
  title: string;
  lead: string;
  heading: string;
  bullets: readonly string[];
  body: string;
  footer: string;
  annotations: readonly PlaceholderAnnotation[];
  shapes: readonly PlaceholderShape[];
};

export const PLACEHOLDER_PAGES: readonly PlaceholderPage[] = [
  {
    id: 1,
    eyebrow: "PDFX 产品团队 · 内部评审稿",
    title: "2026 产品路线图",
    lead: "把 PDF 编辑从“上传、处理、下载”的往返流程，变成在本机完成的一次连续操作。",
    heading: "今年的三个承诺",
    bullets: [
      "本地优先：文档始终留在设备上。",
      "原生注释：高亮与便签可被其他阅读器识别。",
      "覆盖式编辑：改字不重排原始版面。",
    ],
    body: "M0 用浏览器原型锁定信息架构；M1 接入 PDFium 与增量保存；M2 补齐形状、页面操作与导出。",
    footer: "第 1 页 / 共 8 页 · 仅供评审",
    annotations: [
      {
        id: "p1-highlight",
        kind: "highlight",
        label: "高亮：上传、处理、下载",
        color: "#f6c945",
        rect: { left: 20.5, top: 10.6, width: 27, height: 3.2 },
        note: "确认“本机完成”的措辞与隐私承诺一致。",
      },
      {
        id: "p1-note",
        kind: "note",
        label: "便签：路线图节奏",
        color: "#f6c945",
        rect: { left: 84, top: 14.5, width: 6.4, height: 4.6 },
        note: "M1 与 M2 的时间盒需要和工程团队再确认。",
      },
    ],
    shapes: [
      {
        id: "p1-rect",
        kind: "rect",
        label: "矩形：三个承诺",
        fill: "none",
        stroke: "#2563eb",
        strokeWidth: 2,
        rect: { left: 8.5, top: 29.5, width: 83, height: 22 },
      },
    ],
  },
  {
    id: 2,
    eyebrow: "设计原则 · 02",
    title: "像网页一样顺手，像桌面一样可靠",
    lead: "工作区参照熟悉的编辑页信息架构：左侧页面、中央画布、右侧属性，工具沿顶部排列。",
    heading: "四条体验原则",
    bullets: [
      "选择即显示：属性面板只呈现与当前对象相关的选项。",
      "操作可预期：缩放、翻页与选择不打断手上的工作。",
      "中文优先：所有界面文案使用简体中文。",
      "失败可恢复：错误提示明确，并给出重试入口。",
    ],
    body: "M0 用占位内容验证布局、密度与键盘可访问性，不接入任何 PDF 引擎。",
    footer: "第 2 页 / 共 8 页 · 仅供评审",
    annotations: [
      {
        id: "p2-underline",
        kind: "underline",
        label: "下划线：左侧页面、中央画布、右侧属性",
        color: "#2563eb",
        rect: { left: 0.5, top: 11.3, width: 56, height: 2.4 },
      },
    ],
    shapes: [
      {
        id: "p2-ellipse",
        kind: "ellipse",
        label: "椭圆：键盘可访问性",
        fill: "none",
        stroke: "#1f8a5b",
        strokeWidth: 2,
        rect: { left: 66, top: 63, width: 22, height: 14 },
      },
    ],
  },
  {
    id: 3,
    eyebrow: "技术说明 · 03",
    title: "文件从不离开这台设备",
    lead: "打开、渲染、编辑与保存全部在本机完成，没有账户、云端处理或遥测。",
    heading: "边界与信任",
    bullets: [
      "源文档只读：编辑器从不直接修改打开时的文件。",
      "输出文档独立：保存生成新的文件，首次保存等同另存为。",
      "恢复文件留在本地：异常退出后可继续上次会话。",
    ],
    body: "Rust 负责文件系统与 PDF 处理，React 只通过单一命令边界发起操作。",
    footer: "第 3 页 / 共 8 页 · 仅供评审",
    annotations: [
      {
        id: "p3-note",
        kind: "note",
        label: "便签：离线保证",
        color: "#f6c945",
        rect: { left: 84, top: 14.5, width: 6.4, height: 4.6 },
        note: "离线保证需要在发布说明中再次确认。",
      },
    ],
    shapes: [
      {
        id: "p3-arrow",
        kind: "arrow",
        label: "箭头：单一命令边界",
        fill: "none",
        stroke: "#d93b30",
        strokeWidth: 3,
        rect: { left: 10, top: 68, width: 27, height: 9 },
      },
    ],
  },
  {
    id: 4,
    eyebrow: "评审 · 04",
    title: "这次原型要回答什么",
    lead: "在接入引擎之前，先用浏览器原型对齐布局、交互与文案，减少返工。",
    heading: "重点检查项",
    bullets: [
      "1440×900 与 1280×720 下没有横向溢出。",
      "五个页签切换时，右侧属性面板与画布工具保持同步。",
      "缩略图、页码与缩放控件可以独立操作。",
      "文字、注释、形状三种选中状态都有对应属性。",
    ],
    body: "评审结论将决定 M1 的工具栏密度与属性面板宽度。",
    footer: "第 4 页 / 共 8 页 · 仅供评审",
    annotations: [
      {
        id: "p4-strikeout",
        kind: "strikeout",
        label: "删除线：横向溢出检查",
        color: "#d93b30",
        rect: { left: 10, top: 27.2, width: 44, height: 2.6 },
      },
    ],
    shapes: [
      {
        id: "p4-line",
        kind: "line",
        label: "直线：属性面板宽度",
        fill: "none",
        stroke: "#4b5563",
        strokeWidth: 2,
        rect: { left: 12, top: 70, width: 34, height: 1 },
      },
    ],
  },
  {
    id: 5,
    eyebrow: "M1 规划 · 05",
    title: "改字，但不重排版面",
    lead: "点击已有文字进入文本框编辑，保留原字体、字号、颜色与位置。",
    heading: "字体策略",
    bullets: [
      "优先复用嵌入字体，缺字形时回退并提示。",
      "新增文字按需生成字体子集，控制文件体积。",
      "文本框向下扩展换行，不自动缩小字号。",
    ],
    body: "删除文本框时真正移除原文字，避免复制粘贴出现重复内容。",
    footer: "第 5 页 / 共 8 页 · 仅供评审",
    annotations: [],
    shapes: [],
  },
  {
    id: 6,
    eyebrow: "M1 规划 · 06",
    title: "注释要能被别的阅读器继续编辑",
    lead: "高亮、下划线、删除线与便签以原生注释对象写入，而不是画在页面上的色块。",
    heading: "验收方式",
    bullets: [
      "保存后在 macOS Preview 与 Acrobat 中检查注释。",
      "注释颜色、不透明度与内容保持一致。",
      "扁平化是显式操作，不默认发生。",
    ],
    body: "跨阅读器往返清单会用固定语料重复验证。",
    footer: "第 6 页 / 共 8 页 · 仅供评审",
    annotations: [],
    shapes: [],
  },
  {
    id: 7,
    eyebrow: "M3 预览 · 07",
    title: "把纸质流程搬进 PDF",
    lead: "表单字段的创建与填写放在 M3，页面旋转、删除、重排与插入空白页放在 M2。",
    heading: "分阶段交付",
    bullets: [
      "M2：页面操作、形状、签名、链接与图章。",
      "M3：文本字段、复选框、单选与下拉。",
      "表单字段必须能被其他阅读器识别并填写。",
    ],
    body: "表单能力是否完整落地，取决于 M3 前的 AcroForm 技术验证。",
    footer: "第 7 页 / 共 8 页 · 仅供评审",
    annotations: [],
    shapes: [],
  },
  {
    id: 8,
    eyebrow: "下一步 · 08",
    title: "从原型走向可用的编辑器",
    lead: "原型通过评审后，M1 聚焦打开、渲染、文本编辑、注释、撤销与保存。",
    heading: "判断完成的标准",
    bullets: [
      "固定的测试语料全部通过命令边界验证。",
      "保存结果在四个阅读器中打开一致。",
      "大文件与异常退出场景有明确表现。",
    ],
    body: "性能阈值在 M1 技术验证中确定，并写回规格。",
    footer: "第 8 页 / 共 8 页 · 仅供评审",
    annotations: [],
    shapes: [],
  },
];

export const PAGE_COUNT = PLACEHOLDER_PAGES.length;

export function getPage(pageNumber: number): PlaceholderPage {
  return PLACEHOLDER_PAGES[pageNumber - 1] ?? PLACEHOLDER_PAGES[0];
}

export type DocumentAnnotation = {
  page: number;
  annotation: PlaceholderAnnotation;
};

export const DOCUMENT_ANNOTATIONS: readonly DocumentAnnotation[] =
  PLACEHOLDER_PAGES.flatMap((page) =>
    page.annotations.map((annotation) => ({ page: page.id, annotation })),
  );
