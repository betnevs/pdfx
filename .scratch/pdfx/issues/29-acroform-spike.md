# 29: AcroForm 创建技术验证

**What to build:** 验证用 lopdf 创建 AcroForm 字段的可行性与工作量：文本、复选框、单选、列表/下拉字段的 `/AcroForm`、`/Fields`、`/DA`、`/DR`、Widget Annotation 与外观流；空签名字段只建结构。产出 go/no-go 与降级方案。

**Blocked by:** 03 (M0→M1 技术验证), 10 (Save As / Save Changes 管线)

**Status:** done

**Spike outcome (2026-10-28):** GO. Report: [docs/spikes/acroform-create.md](../../../docs/spikes/acroform-create.md). Probe: [spikes/acroform-create/](../../../spikes/acroform-create/).
Reader coverage: Chrome 156 display and fill verified (text and checkbox); PDFKit (the Preview engine) display and field state verified; Preview.app UI not verified because the GUI session was locked; Adobe Acrobat Reader is not installed. See the report's reader table before claiming all three named readers.

- [x] 能创建文本、复选框、单选、列表/下拉字段；Chrome 显示/填写与 PDFKit 显示已验（Preview.app UI 与 Acrobat 未验，见报告）。
- [x] 复选框/单选的 `/Yes`、`/Off`、radio 的 `/Kids` 与 `/AS` 正确。
- [x] 能创建空签名字段结构；不执行证书签名。
- [x] 记录字段外观流、字段名唯一性、旋转/CropBox 坐标与合并已有 AcroForm 的坑。
- [x] 输出 go/no-go：GO，按 31–33 实现；无需降级为“只填不建”。
