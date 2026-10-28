# 11: CJK 字体回退与子集嵌入

**What to build:** 中文与混排文字可以编辑和新增；优先复用原 Embedded Font，缺字形时回退到内置字体并提示；保存时生成 Font Subset，保留 GID 映射与 ToUnicode，文件不过度膨胀。

**Blocked by:** 09 (新增 Text Box 与样式), 10 (Save As / Save Changes 管线)

**Status:** ready-for-agent

- [ ] 编辑/新增中文文字后，画布显示正确，保存后 PDFium 能提取到原文。
- [ ] 原字体缺字形时回退到内置可自由嵌入字体，并在属性面板提示。
- [ ] Font Subset 的 CIDToGIDMap、/W 宽度与 ToUnicode CMap 正确；保存后文字可搜索/复制。
- [ ] 重复保存同一 Document 不会重复嵌入相同子集，文件大小增长受控。
- [ ] 保存结果在 Preview、Adobe Acrobat Reader、Chrome、Edge 中显示一致。
