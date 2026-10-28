# 29: AcroForm 创建技术验证

**What to build:** 验证用 lopdf 创建 AcroForm 字段的可行性与工作量：文本、复选框、单选、列表/下拉字段的 `/AcroForm`、`/Fields`、`/DA`、`/DR`、Widget Annotation 与外观流；空签名字段只建结构。产出 go/no-go 与降级方案。

**Blocked by:** 03 (M0→M1 技术验证), 10 (Save As / Save Changes 管线)

**Status:** ready-for-agent

- [ ] 能创建文本、复选框、单选、列表/下拉字段，并在 Preview、Adobe Acrobat Reader、Chrome 中正确显示与填写。
- [ ] 复选框/单选的 `/Yes`、`/Off`、radio 的 `/Kids` 与 `/AS` 正确。
- [ ] 能创建空签名字段结构；不执行证书签名。
- [ ] 记录字段外观流、字段名唯一性、旋转/CropBox 坐标与合并已有 AcroForm 的坑。
- [ ] 输出 go/no-go：可行则按 31–33 实现；不可行则降级为“只填不建”并更新规格。
