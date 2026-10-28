# PDFium + lopdf 混合 PDF 架构

PDFium 负责渲染、逐字文本/字体/位置提取、页面操作，以及它支持的 Native Annotation 创建；lopdf 负责 PDFium 不支持的对象级编辑、AcroForm 字段创建、页面树/元数据/内容流修补与增量保存；字体管线用 rustybuzz 做排版，subsetter + ttf-parser 做 Font Subset，printpdf 仅作参考。PDFium 无法创建 AcroForm 字段，原生注释只覆盖高亮、下划线、删除线、波浪线、便签、手绘、矩形、椭圆、图章、链接、附件，线/箭头/多边形/云线只能作为页面内容；且 pdfium-render 的高层保存会整份重写；在零成本与保存保真的约束下，混合架构是唯一可行路线。代价是两套 PDF 库之间的对象协调与更复杂的保存路径，必须先用技术验证确认。

pdfium-render 0.9.4 的高层保存硬编码 `flags = 0`，且未暴露文档句柄，因此 `FPDF_INCREMENTAL` 不可达；同时缺少圆形注释创建、墨迹笔画、边框宽度与自定义外观 API。实现选用最小 vendored patch：补丁仅改 5 个文件，新增带 flags 的保存入口、圆形注释创建、墨迹笔画、边框宽度、自定义 `/AP` 与创建句柄的显式关闭；crate 版本与校验和固定在 `src-tauri/vendor/pdfium-render/PDFX-PATCH.md`，`[patch.crates-io]` 指向仓内副本，补丁只经 PDFium 适配层暴露，命令边界不出现 PDFium 类型。`FPDF_REMOVE_SECURITY` 仅用于加密源的全量去密重写。适配层保证增量保存与编辑操作作用于同一个 `PdfDocument` 句柄。
