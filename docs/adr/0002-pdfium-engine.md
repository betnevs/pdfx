---
Status: superseded by ADR-0010
---

# 以 PDFium 作为唯一的 PDF 引擎

渲染、文本提取、注释与表单字段全部基于 PDFium，通过 `pdfium-render` 从 Rust 调用。产品是闭源商业软件，依赖必须限定在 MIT/Apache/BSD 之内，因此 MuPDF 的 AGPL 许可直接排除，尽管它的编辑 API 很完整。PDFium 的 BSD 许可、Chrome 级渲染质量与跨平台预编译产物，使其成为该约束下唯一成熟的引擎选择。
