# PDFX — 跨平台 PDF 编辑器规格

Status: ready-for-agent

## Problem Statement

我需要一个在 macOS 和 Windows 上离线运行的 PDF 编辑器，操作方式和我熟悉的 iLovePDF 编辑页一致：打开本地 PDF，直接改文字、加注释、形状、图片、签名和表单，然后保存。网页版要上传文件、依赖网络和账户，桌面替代品要么价格高、要么交互不熟。我希望用 Rust 技术栈做一个自己的客户端，先把核心编辑体验做到位，再逐步补齐高级能力；全程本地处理，不产生分发成本。

## Solution

PDFX 是一个 Tauri 2 桌面应用：Rust 负责 PDF 处理与文件系统，React/TypeScript 负责界面。工作区复刻 iLovePDF 编辑页的信息架构——顶部“注释/编辑”模式切换、五个页签（注释、形状、插入、编辑文本、表单）、左侧缩略图栏、中央画布、右侧属性面板、底部页码与缩放条——但使用自有品牌。

交付分四段：

- **M0**：浏览器里的 React 高保真原型，对齐布局、交互与中文文案，不接 PDF 引擎。
- **M1**：打开、渲染、缩放、翻页、缩略图；覆盖式文本编辑；添加文本与图片；高亮、下划线、删除线、便签、手绘与注释列表；撤销/重做；崩溃恢复；保存/另存为（Native Annotation、保存保真）；加密 PDF 打开；≤200MB / ≤2000 页。
- **M2**：形状（矩形、椭圆、直线、箭头）、选区级混合样式、图像签名、链接、图章、页面旋转/删除/重排/插入空白页、打印、导出 PNG/JPG、深色模式。
- **M3**：Form Field 创建与填写、高级注释与形状、附件、水印、图层。

## User Stories

### 打开与文档

1. 作为 PDFX 用户，我想要通过文件对话框打开本地 PDF，以便在不联网、不上传的情况下编辑 Document。
2. 作为 PDFX 用户，我想要把 PDF 拖进窗口打开，以便快速开始。
3. 作为 PDFX 用户，我想要从最近文件列表重新打开 Document，以便继续上次的工作。
4. 作为 PDFX 用户，我想要在打开带 User Password 的 PDF 时输入密码，以便访问我有权限的内容。
5. 作为 PDFX 用户，我想要在 Owner Password 施加 Permission Restrictions 时看到明确的只读提示，以便知道为什么不能编辑。
6. 作为 PDFX 用户，我想要打开 Scanned PDF 并添加 Annotation，同时被告知不能编辑文字，以便不误以为功能损坏。
7. 作为 PDFX 用户，我想要打开 200MB / 2000 页的 Document 时先看到第一页、其余页面渐进渲染，以便不用等待整份文件。
8. 作为 PDFX 用户，我想要在关闭有未保存修改的 Document 时被询问，以便避免丢工作。
9. 作为 PDFX 用户，我想要一个窗口只编辑一个 Document，打开新文件时开新窗口，以便和 iLovePDF 的单文档体验一致。
10. 作为 PDFX 用户，我想要在标题栏看到 Document 名称与未保存标记，以便知道当前状态。
11. 作为 PDFX 用户，我想要在设置里选择是否把 PDFX 注册为系统 PDF 打开方式，以便不抢占现有阅读器。
12. 作为 PDFX 用户，我想要在打开带数字签名的 PDF 时看到“编辑会使签名失效”的提示，以便避免误操作。

### 工作区与导航

13. 作为 PDFX 用户，我想要 iLovePDF 式的工作区布局，以便不用重新学习操作。
14. 作为 PDFX 用户，我想要在“注释”和“编辑”模式之间切换，以便区分两类工作。
15. 作为 PDFX 用户，我想要注释、形状、插入、编辑文本、表单五个页签，以便按任务找到工具。
16. 作为 PDFX 用户，我想要 10%–4000% 缩放与“适合宽度/适合页面”，以便查看细节或整页。
17. 作为 PDFX 用户，我想要页码跳转和上一页/下一页，以便快速定位。
18. 作为 PDFX 用户，我想要左侧缩略图栏懒加载并支持点击跳转，以便浏览长文档。
19. 作为 PDFX 用户，我想要原生标题栏与系统菜单（文件/编辑/视图/帮助），以便使用桌面惯例和快捷键。
20. 作为 PDFX 用户，我想要浅色主题，并在 M2 跟随系统深色模式，以便长时间使用舒适。
21. 作为 PDFX 用户，我想要简体中文界面，并让文案集中管理以便后续加英文。
22. 作为 PDFX 用户，我想要常用快捷键（打开、保存、撤销、重做、缩放、翻页），以便高效操作。
23. 作为 PDFX 用户，我想要全屏查看模式，以便专注阅读和编辑。
24. 作为 PDFX 用户，我想要右侧属性面板随选中对象变化（文本、Annotation、Shape），以便只看到相关选项。
25. 作为 PDFX 用户，我想要底部显示当前页/总页数与缩放比例，以便知道位置。
26. 作为 PDFX 用户，我想要可展开的 Annotation 列表，点击条目定位到页面位置，以便管理注释。
27. 作为 PDFX 用户，我想要清晰的错误提示与重试入口，以便知道保存或打开失败时发生了什么。

### 文本编辑

28. 作为 PDFX 用户，我想要点击已有文字进入 Text Box 编辑，以便像 iLovePDF 一样直接改字。
29. 作为 PDFX 用户，我想要 Text Box 保留原 Text Object 的字体、字号、颜色与位置，以便打开时不跳版。
30. 作为 PDFX 用户，我想要在 M1 对整框修改字体、字号、粗体/斜体/下划线、颜色与对齐，以便快速调整。
31. 作为 PDFX 用户，我想要在 M2 对选中部分文字做混合样式，以便精细排版。
32. 作为 PDFX 用户，我想要新增 Text Box，以便在页面上添加文字。
33. 作为 PDFX 用户，我想要 Text Box 在输入超长时自动向下扩展并换行，而不是自动缩小字号，以便保持版式可预期。
34. 作为 PDFX 用户，我想要手动调整 Text Box 大小，只有主动缩小时才裁剪内容，以便控制布局。
35. 作为 PDFX 用户，我想要原字体缺字形时回退到内置字体并收到提示，以便知道显示为什么变了。
36. 作为 PDFX 用户，我想要编辑已有文字时优先复用 Embedded Font，新文字按需生成 Font Subset，以便文件不过度膨胀。
37. 作为 PDFX 用户，我想要删除 Text Box 时真正移除原文字（而不是白色遮盖），以便复制粘贴不出现重复文字。
38. 作为 PDFX 用户，我想要在 Scanned PDF 上禁用文字编辑并显示原因，以便只用 Annotation。
39. 作为 PDFX 用户，我想要移动和调整 Text Box，以便修正位置。
40. 作为 PDFX 用户，我想要在 M2 编辑超链接，以便更新文档里的链接目标。
41. 作为 PDFX 用户，我想要撤销和重做所有文本编辑，以便放心尝试。

### 注释

42. 作为 PDFX 用户，我想要高亮选中文字，以便标记重点。
43. 作为 PDFX 用户，我想要下划线与删除线，以便做校对标记。
44. 作为 PDFX 用户，我想要在页面任意位置放置便签，以便写备注。
45. 作为 PDFX 用户，我想要手绘自由线条，以便圈画或签名式标注。
46. 作为 PDFX 用户，我想要在 Annotation 列表里定位、选择和删除注释，以便集中管理。
47. 作为 PDFX 用户，我想要 Annotation 自动记录作者与时间，并可在设置里改显示名，以便其他阅读器里看起来完整。
48. 作为 PDFX 用户，我想要调整 Annotation 的颜色、透明度与线宽，以便符合文档风格。
49. 作为 PDFX 用户，我想要 Annotation 保存为 Native Annotation，以便在 Preview、Adobe Reader、Chrome 里仍可识别和编辑。
50. 作为 PDFX 用户，我想要在 M3 把 Annotation Flatten 成普通页面内容并导出副本，以便任何阅读器显示一致。
51. 作为 PDFX 用户，我想要在 M3 使用波浪线、自由文本、标注框与橡皮擦，以便覆盖更细的批注场景。
52. 作为 PDFX 用户，我想要移动和调整便签类 Annotation，以便修正位置。

### 形状

53. 作为 PDFX 用户，我想要在 M2 绘制矩形、椭圆、直线与箭头，以便框选或指示内容。
54. 作为 PDFX 用户，我想要调整 Shape 的填充、描边、透明度与线宽，以便匹配文档视觉。
55. 作为 PDFX 用户，我想要移动、缩放与旋转 Shape，以便精确摆放。
56. 作为 PDFX 用户，我想要在 M3 使用弧线、多边形、折线与云线，以便做更复杂的标注。

### 插入

57. 作为 PDFX 用户，我想要插入 PNG/JPG 图片，并调整大小与旋转，以便把图片放进页面。
58. 作为 PDFX 用户，我想要在 M2 绘制、输入或上传签名图片，以便签署文档。
59. 作为 PDFX 用户，我想要在 M2 添加或编辑超链接，以便把文字或图片链到网址或页面。
60. 作为 PDFX 用户，我想要在 M2 插入图章，以便模拟盖章。
61. 作为 PDFX 用户，我想要在 M3 添加文件附件，以便把相关文件随 PDF 一起保存。
62. 作为 PDFX 用户，我想要在 M3 添加水印，以便标记文档状态。

### 表单

63. 作为 PDFX 用户，我想要在 M3 填写已有的 Form Field，以便完成表单。
64. 作为 PDFX 用户，我想要在 M3 创建文本、复选框、单选、列表、下拉与空签名字段，以便制作可填写表单。
65. 作为 PDFX 用户，我想要设置 Form Field 的名称、提示、必填、多行与字号，以便控制填写体验。
66. 作为 PDFX 用户，我想要表单不执行计算或 JavaScript 动作，以便避免脚本安全与兼容性问题。
67. 作为 PDFX 用户，我想要保存后的 Form Field 能被其他阅读器识别和填写，以便把表单交给别人。
68. 作为 PDFX 用户，我想要必填字段在导出前得到提示，以便避免漏填。

### 页面管理

69. 作为 PDFX 用户，我想要在 M2 旋转 Page，以便修正扫描方向。
70. 作为 PDFX 用户，我想要在 M2 删除 Page，以便移除不需要的页面。
71. 作为 PDFX 用户，我想要在 M2 重排 Page，以便调整顺序。
72. 作为 PDFX 用户，我想要在 M2 插入空白 Page，以便补充内容。
73. 作为 PDFX 用户，我想要在 M3 复制 Page，以便复用版式。
74. 作为 PDFX 用户，我想要缩略图栏在页面操作后立即更新，以便确认结果。

### 保存

75. 作为 PDFX 用户，我想要首次保存时走 Save As，以便选择 Output Document 的位置。
76. 作为 PDFX 用户，我想要之后 Save Changes 覆盖当前 Output Document，以便快速迭代。
77. 作为 PDFX 用户，我想要 Source Document 永远不被修改，以便保留原始文件。
78. 作为 PDFX 用户，我想要覆盖 Output Document 前自动备份，以便误保存后可恢复。
79. 作为 PDFX 用户，我想要保存时保留书签、链接、元数据、已有 Form Field 与未改动的页面内容，以便不破坏原文档结构。
80. 作为 PDFX 用户，我想要 Annotation 保存为 Native Annotation，以便其他阅读器可见可编辑。
81. 作为 PDFX 用户，我想要打开加密 PDF 后保存时得到“输出将不加密”的明确提示，以便我知道安全状态。
82. 作为 PDFX 用户，我想要在 M3 选择给输出设置打开密码，以便保护结果。
83. 作为 PDFX 用户，我想要保存失败时源文件与已有输出保持完整，以便不丢数据。
84. 作为 PDFX 用户，我想要大文件保存显示进度，以便知道还要等多久。
85. 作为 PDFX 用户，我想要在 M3 导出 Flatten 副本，以便获得任何阅读器都一致显示的文件。
86. 作为 PDFX 用户，我想要在 M2 打印 Document，以便纸质输出。
87. 作为 PDFX 用户，我想要在 M2 把当前 Page 导出为 PNG/JPG，以便插入其他文档或分享。
88. 作为 QA，我想要保存结果在 Preview、Adobe Acrobat Reader、Chrome、Edge 中往返验证，以便确认跨阅读器一致。

### 撤销、恢复与最近文件

89. 作为 PDFX 用户，我想要所有编辑动作走统一撤销/重做栈，以便行为一致。
90. 作为 PDFX 用户，我想要保存后仍能撤销到保存前，以便继续调整。
91. 作为 PDFX 用户，我想要编辑时自动写入 Recovery File，以便异常退出后恢复。
92. 作为 PDFX 用户，我想要下次启动时被提示恢复上次会话，以便找回工作。
93. 作为 PDFX 用户，我想要 Recovery File 永不覆盖 Source Document 或 Output Document，以便不污染正式文件。
94. 作为 PDFX 用户，我想要最近文件列表，以便快速回到常用文档。

### 设置与隐私

95. 作为 PDFX 用户，我想要在设置里改界面语言、主题、Annotation 显示名与文件关联开关，以便按偏好使用。
96. 作为 PDFX 用户，我想要应用不发送遥测、不上传崩溃报告、不主动联网，以便文件与使用行为留在本地。
97. 作为 PDFX 用户，我想要本地滚动日志，并能手动导出诊断包，以便排查问题而不自动上传。
98. 作为 PDFX 用户，我想要只在主动点击“检查更新”时联网，并打开下载页，以便控制网络行为。
99. 作为 PDFX 用户，我想要应用不自动下载或安装更新，以便每次升级都由我确认。
100. 作为 PDFX 用户，我想要设置项在本地持久化，以便重启后保留。

### 平台与分发

101. 作为 macOS 用户，我想要 macOS 12+ 的 universal 构建（Apple Silicon + Intel），以便在新旧机器上运行。
102. 作为 Windows 用户，我想要 Windows 10 1809+ / 11 x64 构建，以便覆盖常见环境。
103. 作为内部用户，我想要未签名的安装包与绕过说明，以便零成本安装。
104. 作为内部用户，我想要 macOS `.dmg`、Windows `.msi`、`.exe` 安装器与 portable zip，以便按场景选择。
105. 作为 Windows 用户，我想要安装包内置 WebView2 Evergreen 离线安装器，以便离线也能完成安装。
106. 作为 PDFX 用户，我想要自有品牌的应用名、图标与配色，以便和 iLovePDF 区分。

### M0 原型与验收

107. 作为项目所有者，我想要 M0 浏览器版 React 原型，包含真实布局、五个页签、属性面板、缩略图栏、底部页码/缩放与中文文案，以便在接引擎前对齐视觉与交互。
108. 作为项目所有者，我想要 M0 用占位 PDF 内容，以便不依赖 PDF 引擎就能评审。
109. 作为 QA，我想要固定测试集（文本型、扫描型、加密型、表单型、大文件、损坏文件），以便重复验证。
110. 作为 QA，我想要 M1 的跨阅读器往返验收清单，以便判断“完成”。
111. 作为开发者，我想要 M0 后 3–5 天的技术验证，确认 Native Annotation 外观流与增量保存可行，以便决定 M1 的保存路径。
112. 作为开发者，我想要 M3 前 3–5 天的 AcroForm 创建验证，以便决定表单创建是否降级为“只填不建”。

## Implementation Decisions

### 运行时与平台

- 桌面壳用 Tauri 2（ADR-0001），前端用 React + TypeScript，Rust 负责 PDF 处理、文件系统与命令编排。
- 目标平台：macOS 12+（Apple Silicon + Intel，universal）、Windows 10 1809+ / 11 x64。
- Windows 安装包内置 WebView2 Evergreen 离线安装器（`offlineInstaller`，每个安装包约 +127MB），保证离线安装；不使用需要联网的 bootstrapper。这是“离线可用”优先于安装包体积的取舍；portable zip 不内置 WebView2，文档注明系统需预装。
- 界面使用原生标题栏与系统菜单；应用内保留编辑器工具栏。主题以浅色为主，M2 跟随系统深色模式。

### PDF 引擎

- 采用 PDFium + lopdf 混合架构（ADR-0010，取代 ADR-0002 的“唯一引擎”）。
- PDFium 通过 `pdfium-render` 负责：页面渲染、逐字 Text Object 提取（字体、字号、颜色、位置）、页面旋转/增删/重排，以及它支持的 Native Annotation 创建（高亮、下划线、删除线、波浪线、便签、手绘、矩形、椭圆、图章、链接、附件）。
- lopdf 负责：PDFium 不支持的对象级编辑、AcroForm 字段创建、页面树/元数据/内容流修补、增量保存、结构保真。
- 线、箭头、多边形、折线、云线不作为 Annotation，而是作为页面内容对象创建；它们没有注释线程、注释选择与标准删除语义。
- 字体管线：rustybuzz 做排版，subsetter + ttf-parser 做 Font Subset，手写 Type0/Identity-H、CIDFontType2、FontDescriptor、FontFile2、ToUnicode CMap 与 /W 宽度；printpdf 只作为 MIT 参考实现，不直接合并其对象。
- 保存是分阶段流水线，且同一 revision 只能有一个写入者：先应用 PDFium 原生编辑并增量保存得到字节，再用 lopdf::IncrementalDocument 载入这些字节追加自己的 revision，或反向执行；绝不让两套引擎同时修改同一页/内容流。

### 文档模型与命令边界

- 领域对象：Document、Page、Text Object、Text Box、Annotation、Shape、Form Field、Edit Session、Recovery File。
- 一个窗口一个 Document；打开新文件开新窗口。
- 唯一主 seam 是 Tauri 命令边界（EditorBackend）。前端与测试都通过同一组命令操作：
  - `open_document(path, password?)`
  - `close_document(document_id)`
  - `render_page(document_id, page_index, scale)`
  - `extract_text(document_id, page_index)`
  - `begin_session(document_id)`
  - `apply_edit(session_id, edit)`
  - `undo(session_id)` / `redo(session_id)`
  - `save_as(session_id, target_path)`
  - `save_changes(session_id)`
  - `page_op(session_id, page_index, operation)`
  - `export_page(session_id, page_index, format)`
  - `check_for_updates()`
- 命令返回结构化的 Document 视图、Page 视图、编辑结果与错误；前端不直接接触 PDFium/lopdf。

### 渲染

- 页面按需渲染：只渲染可见页与邻近页；缩略图单独低分辨率渲染并懒加载。
- 大文件（≤200MB / ≤2000 页）目标：首屏先出第一页，滚动与编辑不阻塞；渲染与保存放到后台线程，前端显示进度。
- 渲染缓存按 Document、Page、缩放级别管理；关闭 Document 时释放。
- 文本层与覆盖层分离：PDFium 渲染底图，Text Box、Annotation、Shape 由前端 DOM/SVG 覆盖，保证编辑响应速度。

### 文本编辑

- Overlay Editing（ADR-0003）：提取原 Text Object 的字体、字号、颜色、位置，生成 Text Box；保存时移除原文字并写入新文字，不做段落 reflow。
- M1 整框统一样式；M2 支持选区级混合样式。
- Text Box 输入超长时自动向下扩展并换行；用户手动缩小时才裁剪；不自动缩小字号。
- 原字体缺字形时回退到内置字体，并在属性面板提示；优先复用 Embedded Font，新文字按需生成 Font Subset。
- Scanned PDF 不提供文字编辑，只保留 Annotation 与 Shape。
- 删除 Text Box 时移除原文字对象，不用白色遮盖。

### 字体

- 内置可自由嵌入的字体（ADR-0006）：Noto Sans SC/Noto Serif SC（思源黑体/思源宋体的 TrueType 构建）、Inter、Noto Serif、Liberation Sans/Serif（作为 Arial/Times New Roman 的度量兼容替代）。CJK 必须使用 TrueType 构建以配合 CIDFontType2 + FontFile2；Adobe 的 CFF OTF 需要单独的 CIDFontType0 管线，不在本规格范围。
- 保存时按需子集嵌入；不调用系统字体选择器，不嵌入许可不明确的系统字体。
- 子集化会重排 glyph ID，必须保留 old-to-new GID 映射，用于 CIDToGIDMap、/W 宽度与 ToUnicode CMap。
- CJK 横排主要依赖 cmap/glyph 选择；竖排（/W2、/DW2）与 Han unification 的语言感知回退不在 M1–M3 范围。
- 字体列表与 iLovePDF 不完全一致，这是零成本复刻的已知差异。

### 注释与形状

- Annotation 默认保存为 Native Annotation（ADR-0005）；M3 提供 Flatten 副本导出。
- M1 支持高亮、下划线、删除线、便签、手绘；M2 支持矩形、椭圆、直线、箭头；M3 支持波浪线、自由文本、标注框、橡皮擦。
- M1/M2 的 Native Annotation 优先用 PDFium 的 `FPDFPage_CreateAnnot` 创建；PDFium 没有通用外观生成器，需要时用 `FPDFAnnot_SetAP` 提供自定义外观流。
- 直线、箭头、多边形、折线、云线作为页面内容对象创建，不是 Annotation。
- Annotation 记录作者（默认系统用户名，可改）与时间；支持列表定位、删除、颜色/透明度/线宽调整。
- Shape 支持填充、描边、透明度、线宽、移动、缩放、旋转。

### 表单

- M3 支持填写已有 Form Field 与创建新字段：文本、复选框、单选、列表、下拉、空签名字段。
- 字段属性：名称、提示、必填、多行、字号。
- 不执行计算或 JavaScript 动作；签名字段只创建结构，不做证书签名。
- PDFium 负责读取与填写；lopdf 负责创建 /AcroForm、/Fields、/DA、/DR、Widget Annotation 与外观流（含 /Yes、/Off 状态、radio 的 /Kids 与 /AS）。
- 创建的字段采用终端 Widget（字段字典与 Widget 为同一对象），并为每个状态写出显式 /AP /N；不依赖 /NeedAppearances 让阅读器生成外观。字符串值（/V、/TU、/Opt 等）使用 UTF-16BE；界面文字与外观使用内置 Noto Sans SC 子集，CJK 字体资源同时写入 /DR 与外观资源字典。
- 页面 /Rotate 为 90/180/270 时，用外观变换矩阵补偿字段坐标，使画布、保存后渲染与其他阅读器中的位置一致。
- 列表/下拉使用 /FT /Ch 与 /Opt、/V、/I；只有列表字段可多选。空签名字段使用 /FT /Sig，只提供可见 /AP，不写 /V、/SV 或 /Lock，并按需维护 /SigFlags（不设置 AppendOnly）；不执行证书签名。
- 创建、属性修改与删除作为单个 lopdf incremental revision 追加到保存结果；重复保存不重复创建字段或字体资源。
- 创建能力必须先通过技术验证；失败则降级为“只填不建”。按调研估算，文本/复选框/单选/下拉的完整实现约 2–4 工程师周；证书签名另需 4–8+ 周，本规格不做。

### 页面管理

- M2 支持旋转、删除、重排、插入空白 Page；M3 支持复制 Page。
- 页面操作通过 lopdf 对象级改写完成，保留未操作页面的内容；缩略图与画布在操作后刷新。

### 保存与保真

- 首次保存走 Save As；之后 Save Changes 覆盖当前 Output Document；Source Document 永不修改。
- 覆盖 Output Document 前自动备份；保存失败时源文件与已有输出保持完整。
- 保存只写入编辑产生的对象，保留书签、链接、元数据、已有 Form Field 与未改动的页面内容（ADR-0004）。
- 使用分阶段增量保存：PDFium 的 `FPDF_SaveAsCopy(..., FPDF_INCREMENTAL)` 追加改动对象与新的 xref/trailer，lopdf 再用 `IncrementalDocument` 追加自己的 revision；同一 revision 只能有一个写入者，绝不并发修改同一页/内容流。
- PDFium 会重写被触碰页面的字典与内容流，因此编辑页上的链接、Annotation 与书签必须用 fixture 做保存前后断言。
- 不用 pdfium-render 高层的整份重写路径。
- 加密 Source Document 保存时默认输出不加密，并在保存前明确提示（ADR-0007）；M3 提供可选打开密码。
- M2 打印与导出当前 Page 为 PNG/JPG；M3 提供 Flatten 副本导出。

### 撤销与恢复

- 所有编辑动作（文本、Annotation、Shape、Form Field、页面操作）进入统一命令栈；保存不切断历史。
- Recovery File 与 Source Document、Output Document 分离；编辑动作节流写入；异常退出后下次启动提示恢复。
- 最近文件列表本地持久化；正常启动不自动打开文档。

### 设置与隐私

- 设置项：界面语言、主题、Annotation 显示名、文件关联开关、更新检查。
- 应用不发送遥测、不上传崩溃报告、不主动联网；唯一联网行为是用户主动点击“检查更新”，请求版本 JSON 并打开下载页。
- 本地滚动日志；诊断包由用户手动导出，不自动上传。
- 文件关联默认关闭，由用户在设置里开启。

### 分发与更新

- 零成本分发（ADR-0009）：不购买 Apple Developer Program 或 Windows 代码签名证书；未签名构建，用户通过系统设置、终端或 SmartScreen 自行绕过。
- 安装包：macOS universal `.dmg`、Windows `.msi` + `.exe`、portable zip。
- 更新采用半自动：用户主动检查版本，提示后打开下载页；不自动下载或安装。
- 公开分发前的签名/公证可在未来追加，不影响编辑架构。

### 里程碑与技术验证

- M0：浏览器版 React 高保真原型；不接 Tauri、不接 PDF 引擎；用占位 PDF 对齐布局、五个页签、属性面板、缩略图栏、底部页码/缩放与中文文案。
- M0 后、M1 前：3–5 天技术验证，确认 Native Annotation 外观流、增量保存、CJK 字形排版与 Font Subset 可行；失败则调整 M1 保存路径并重新确认。
- M1、M2、M3 范围见 User Stories 与 Solution。
- M3 前：3–5 天 AcroForm 创建验证；完整实现按调研估算约 2–4 工程师周；失败则降级为“只填不建”。

### ADR

- ADR-0001：Tauri 2 + Web 前端。
- ADR-0002：PDFium 唯一引擎（已被 ADR-0010 取代）。
- ADR-0003：覆盖式文本编辑，不重排段落。
- ADR-0004：保存时保留未编辑的 PDF 结构。
- ADR-0005：注释默认写为 Native Annotation，Flatten 仅作导出选项。
- ADR-0006：只内置可自由嵌入的字体。
- ADR-0007：加密 PDF 保存时默认输出不加密。
- ADR-0008：内部阶段不签名、手动更新（已被 ADR-0009 取代）。
- ADR-0009：零成本分发与半自动更新。
- ADR-0010：PDFium + lopdf 混合架构。

## Testing Decisions

### 主 seam

唯一主 seam 是 **Tauri 命令边界（EditorBackend）**。前端和测试都通过同一组命令操作 Document：打开、渲染、提取文字、开始会话、应用编辑、撤销/重做、保存、页面操作、导出、检查更新。测试不直接调用 PDFium 或 lopdf，也不通过 UI 点击验证核心行为；引擎差异由该 seam 的往返断言覆盖。

UI 冒烟测试（Playwright 驱动 M0 原型）只验证布局、页签切换、面板联动与中文文案，不作为正确性 seam。

### 测试语料

固定 fixture 集，覆盖：

- 文本型 PDF：拉丁、中文、混排、Embedded Font、Font Subset、CID/Type0 字体。
- Scanned PDF：纯图片页。
- 加密 PDF：User Password；Owner Password 限制编辑。
- 表单 PDF：已有 AcroForm 字段。
- 结构 PDF：书签、链接、元数据、附件、已有 Annotation。
- 大文件：≤200MB / ≤2000 页。
- 旋转页、混合页面尺寸、透明与图片。
- 损坏/畸形 PDF。
- 带数字签名的 PDF（提示行为待确认）。
- 编辑页上的链接、Annotation、书签与表单字段（保存前后对比）。
- 各类 Annotation：高亮、下划线、删除线、波浪线、便签、手绘、矩形、椭圆、图章、链接、附件。
- AcroForm 的 Widget 外观流、/Yes 与 /Off 状态、radio 的 /Kids 与 /AS。
- Font Subset 的 GID 重映射与 ToUnicode。

### 断言方式

- 保存结果用独立的 lopdf 解析断言：页数、旋转、书签、链接、元数据、Annotation 对象、Form Field、文字存在/不存在、原内容是否保留。
- 用 PDFium 以固定 DPI 渲染保存前后页面，做快照对比（允许抗锯齿容差）。
- 文本往返：编辑后的文字能被提取到；原文字不重复出现；Scanned PDF 不产生文字层。
- 加密与权限：只读文档拒绝编辑；输出不加密时有明确警告。
- 恢复：模拟异常退出，重启后能恢复 Recovery File。
- 性能：首屏时间、缩放/滚动响应、保存耗时、内存峰值；阈值在 M1 技术验证中确定。

### 跨阅读器验收

M1 起，每个里程碑的保存结果在 macOS Preview、Adobe Acrobat Reader、Chrome、Edge 中打开验证；Windows 侧在用户提供的机器/虚拟机上按清单验收。

### 模块与先例

- 被测试的模块：命令边界、保存管线、文本提取与覆盖编辑、Annotation、Form Field、页面操作、恢复、加密与权限。
- 仓库是 greenfield，没有既有测试先例；采用 Rust 集成测试 + 固定 fixture + 快照测试。后续测试应复用同一套 fixture 与命令驱动方式，不再为单个模块新开 seam。

## Out of Scope

- OCR（iLovePDF 的 OCR 是独立工具）。
- 云存储（Google Drive / Dropbox）、账户、登录、Premium 付费墙。
- 协作、评论回复/线程、多人同时编辑。
- 服务端处理；PDFX 全程本地。
- 证书数字签名（本规格的“签名”是图像签名；表单里的空签名字段只建结构，不执行签名）。
- 日期/页码插入（未在 iLovePDF 编辑页确认）。
- 涂黑/Redaction（截图中的红色图标待确认；确认前不做）。
- 自动下载/安装更新（只做半自动检查 + 打开下载页）。
- Apple 公证、Windows 代码签名（零成本分发阶段不做）。
- PDF/A、无障碍标签、PDF 组合包（portfolio）。
- JavaScript 表单计算与动作。
- 单页/双页/封面等页面布局模式。
- 多文档标签页（一个窗口一个 Document）。

## Further Notes

- iLovePDF 的编辑器是 Apryse WebViewer 的 OEM 版（TrueEdit）；内容编辑、注释、表单、图层来自商业 SDK。用开源 Rust 栈做完整 1:1 不现实，本规格的目标是“核心编辑体验一致，高级能力分批补齐”。
- 字体列表无法与 iLovePDF 完全一致：对方使用 Arial/Times/Courier/Verdana/Impact/Comic Sans 等商业字体，PDFX 只内置可自由嵌入的开源字体。
- 调研代理的后续细节（PDFium Annotation 创建范围、AcroForm 创建、增量保存、CJK 字体管线）已并入本规格；结论会用于 M0 后的技术验证。
- 调研确认：PDFium 的 Annotation 白名单为高亮、下划线、删除线、波浪线、便签、手绘、矩形、椭圆、图章、链接、附件；线/箭头/多边形/云线只能作为页面内容，不是 Annotation。
- AcroForm 创建约 2–4 工程师周；签名字段只建结构，证书签名不做。增量保存是顺序追加，同一 revision 只能有一个写入者。
- 带数字签名的 PDF 在编辑后签名会失效；当前规格只要求提示，具体提示文案与是否阻止保存在 M3 前确认。
- `docs/agents/` 的 issue tracker 与 triage label 配置尚未落盘；建议运行 `/setup-matt-pocock-skills` 完成配置，否则后续 `/triage`、`/to-tickets` 没有正式约定可读。本规格按之前确认的本地 Markdown 约定发布在 `.scratch/pdfx/spec.md`，状态为 `ready-for-agent`。
- M0 原型是一次性设计产物，不进入生产代码；确认视觉与交互后再接 Tauri 与 PDF 引擎。
- 本规格覆盖 M0–M3；拆票时按里程碑与 User Stories 分组，优先 M0 与 M1。
