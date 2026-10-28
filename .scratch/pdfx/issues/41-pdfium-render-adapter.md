# 41: PDFium 增量保存与注释底层适配

**What to build:** 一个生产可用的 PDFium 适配层，补齐 `pdfium-render` 0.9.4 缺失的能力：带 flags 的保存（`FPDF_INCREMENTAL` / `FPDF_REMOVE_SECURITY`）、圆形注释创建、墨迹笔画、边框宽度与自定义 `/AP`。实现可以是最小 vendored patch，也可以是受控 raw FFI；必须隔离在适配层内，升级 PDFium 时只需改这一处。

**Blocked by:** 03 (M0→M1 技术验证), 04 (打开文档并渲染第一页)

**Status:** ready-for-agent

- [ ] 保存入口可显式选择增量保存与全量重写；增量保存的输出以原始字节为前缀，并保留书签、链接、元数据与未编辑页面。
- [ ] 圆形注释、墨迹笔画、边框宽度与自定义 `/AP` 通过适配层可用；保存并重新打开后可见。
- [ ] 适配层不把 PDFium 类型泄露到命令边界，调用方不需要 `unsafe`。
- [ ] 在 pinned PDFium 151.0.7881.0 上通过集成测试；若采用 vendored patch，记录补丁范围、校验方式与升级步骤。
- [ ] 加密源的全量去密重写路径有测试或明确标注为后续票的阻塞项，不静默产生加密输出。
