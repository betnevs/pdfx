# 29: AcroForm 创建技术验证

**What to build:** 验证用 lopdf 创建 AcroForm 字段的可行性与工作量：文本、复选框、单选、列表/下拉字段的 `/AcroForm`、`/Fields`、`/DA`、`/DR`、Widget Annotation 与外观流；空签名字段只建结构。产出 go/no-go 与降级方案。

**Blocked by:** 03 (M0→M1 技术验证), 10 (Save As / Save Changes 管线)

**Status:** done

**Spike outcome (2026-10-28):** GO. Report: [docs/spikes/acroform-create.md](../../../docs/spikes/acroform-create.md). Probe: [spikes/acroform-create/](../../../spikes/acroform-create/).
Reader coverage: Chrome 156 display and fill verified (text and checkbox); PDFKit (the Preview engine) display and field state verified; Preview.app UI not verified because the GUI session was locked; Adobe Acrobat Reader is not installed. See the report's reader table before claiming all three named readers.

- [x] 能创建文本、复选框、单选、列表/下拉字段；PDFium/PDFKit 显示与 Chrome 显示已验，Chrome 交互仅验文本与复选框；Preview.app UI 与 Adobe Acrobat Reader 未验（见报告）。
- [x] 复选框/单选的 `/Yes`、`/Off`、radio 的 `/Kids` 与 `/AS` 正确。
- [x] 能创建空签名字段结构；不执行证书签名。
- [x] 记录字段外观流、字段名唯一性、旋转/CropBox 坐标与合并已有 AcroForm 的坑。
- [x] 输出 go/no-go：GO，按 31–33 实现；无需降级为“只填不建”。

## Verification notes

**Merge:** `da62ed7` `merge: ticket 29 acroform spike`（分支 `ticket/29-acroform-spike` @ `f039a48`，integration 基线 `e3f837d`）。合并内容仅限 spike、报告、本票与 fixture；未改应用代码、`src-tauri/Cargo.lock`、打包或规格。

**Spike 测试：** 在 `spikes/acroform-create` 运行 `cargo test` → 15 passed / 0 failed（creation 8、fixtures 3、flatten 1、geometry 2、unicode 1）；`cargo fmt --check` 通过。`cargo run` 连续运行两次，四个 fixture 的 SHA-256 完全一致且与报告、已提交文件相符，确定性成立。

**合并后整树门禁：** `npm run typecheck` 通过；`npm test` 20 个文件 / 215 tests passed；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` 退出码 0（仅有 vendored `pdfium-render` 依赖的 warning）；`cargo test` 152 passed / 0 failed。

**阅读器证据：** Chrome 156.0.8078.48（本机安装版，headless）显示全部字段，并成功把文本值改为 `Ada Lovelace-Chrome`、切换复选框；PDFKit（Preview 引擎）显示全部字段并读回所有值与状态。截图已提交于 `spikes/acroform-create/reader/evidence/`。Preview.app UI 因 GUI 会话锁定未验，Adobe Acrobat Reader 未安装未验；未升级这两项结论。

**GO：** 判定 GO，31–33 可按规格实现；规格 2–4 工程师周估算成立（报告细化为 12–18 工程师日，约 2.5–3.5 周）。无需降级为“只填不建”。

**带入 31–33 的遗留项：**
- 旋转页的 `/Rect` 映射正确（geometry 测试覆盖 0/90/180/270 与 CropBox），但 AP 内容未做旋转补偿：90/180/270 下文本、勾选与选项值在 PDFKit/Chrome 中均为横向/竖排。31 需实现 AP 旋转矩阵并补 90/180/270 截图。
- 非 ASCII `/V` 以 UTF-16BE+BOM 正确往返，但外观流使用 WinAnsi Helvetica，无法绘制 CJK；需要规格已要求的嵌入子集字体管线。
- 空签名字段只有结构、无 `/AP`，阅读器不画可见框；33 需决定是否加带边框的空外观或 `/MK` border。
- 生成的表单不得设置 `/NeedAppearances`（默认 false，Chrome/PDFKit 均可显示与编辑）；合并已有表单时必须保留原值。应用侧 PDFium 页面渲染还需处理源文件为 true 的情况（widget 会走缺少平台回调的 form-fill 路径）。
- 每个 widget 必须有显式 `/AP /N`；多行外观仅置 `Ff` bit 12 而未实现换行布局；flatten 会丢弃无 `/AP` 的 widget，且源表单 `NeedAppearances true` 时可能烘焙过期外观；radio on-state 值由标签派生，父 `/V`、kid `/AS` 与 `/AP` 状态名必须一致；合并时保留已有 AcroForm 字典，仅对新名称去重。

**规格/ADR：** 未修改。报告建议在规格 `### 表单` 下补充三条约束（每 widget 显式 `/AP` 与 `/NeedAppearances` 策略；`/Rect` 用户空间与旋转补偿；UTF-16BE `/V` 与 CJK 子集外观字体），由团队负责人决定是否采纳。
