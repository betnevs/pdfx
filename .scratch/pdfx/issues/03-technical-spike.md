# 03: M0→M1 技术验证

**What to build:** 一个可运行的探针，验证 PDFium + lopdf 的关键路径：打开并渲染 PDF、逐字提取、创建白名单内的 Native Annotation、PDFium 增量保存后由 lopdf 追加 revision、CJK 字形排版与 Font Subset 嵌入。产出一页 go/no-go 结论，决定 M1 的保存、注释与字体路径。

**Blocked by:** 01 (Repo + Tauri/React 脚手架 + 测试与 fixture 骨架)

**Status:** ready-for-agent

- [ ] 探针能打开文本型、Scanned、加密、带书签/链接的 PDF，并输出每页渲染结果与文本提取结果。
- [ ] 能创建高亮、下划线、删除线、便签、手绘、矩形、椭圆 Native Annotation，并在保存后由 PDFium 重新打开可见。
- [ ] 能完成一次“PDFium 增量保存 → lopdf 追加 revision”的顺序保存，保存后原书签、链接、元数据仍存在。
- [ ] 能嵌入一个 CJK Font Subset，保存后文字可被 PDFium 提取，且 ToUnicode 与 GID 映射正确。
- [ ] 输出书面结论：哪些路径可行、哪些降级、M1 的保存与字体方案；失败项列入后续票的风险。
