# 33: 创建列表/下拉与空签名字段

**What to build:** 用户可以创建列表/下拉 Form Field 与空签名字段；签名字段只建结构，不执行证书签名。保存后其他阅读器能识别字段。

**Blocked by:** 31 (创建文本字段)

**Status:** ready-for-agent

- [ ] 列表/下拉字段可设置选项、默认值、多选（列表）与字号。
- [ ] 空签名字段可放置并设置名称；保存后阅读器显示签名区域。
- [ ] 不执行证书签名；UI 明确说明签名字段只提供结构。
- [ ] 保存后在 Preview、Adobe Acrobat Reader、Chrome 中字段结构可识别。
- [ ] 撤销/重做覆盖创建、属性修改与删除。

**Verification (agent):** 分支 `ticket/33-create-choice-signature-fields`，功能提交 `ae27bfa`，合入 integration tip `0650973` 后为 `b8675e7`（基于 `03d8f56`；合并无冲突，`PDFX-PATCH.md` 与 integration tip 完全一致，本票没有新增 vendored hunk）。相对 integration tip 的代码改动为 9 个文件（+3315/−199）：`form_create.rs`、`form.rs`、`tests/form_creation.rs`、`tests/form_fields.rs`、`FormLayer.tsx`、`FormLayer.test.tsx`、`backend.ts`、`backend.test.ts`、`styles.css`；未改 spec、ADR、`Cargo.toml`/`Cargo.lock`/`package.json`/`package-lock.json`、`tauri.conf`、capabilities，未新增依赖。

**实现：** 列表/下拉复用 ticket 31/32 的 `CreatedFieldStore`、`FormFieldSpec`（新增 `Choice`/`Signature` 变体）、`ObjectEdit::CreateFormFields`、单一 writer `apply_created_fields`、`EditAction::FormStructure` 与单次 lopdf 增量 revision。choice 写 `terminal widget`：`/FT /Ch`、唯一 `/T`、`/Rect`、`/P`、`/F 4`、`/Ff`（Required bit 2；下拉 Combo bit 18；列表 MultiSelect bit 22，仅列表）、`/Opt`（UTF-16BE 文本串）、`/V`（单个选中写文本串，多选写文本数组，无默认省略）、`/I`（选中索引数组）、`/DA`（含字号）、`/Q 0`、`/Border`、显式 `/AP /N`；外观流用嵌入的 Noto Sans SC 子集绘制选中项（下拉含箭头与空态，列表逐行绘制选项并高亮选中项），字体同时进入 `/DR` 与 AP resources。签名字段写 `/FT /Sig`、唯一 `/T`、`/Rect`、`/P`、`/F 4`、`/Ff 0`、`/AP /N`（缓存标签“签名”、边框与签名线）与可选 `/TU`；刻意不写 `/V`、`/SV`、`/Lock`、`/AA`，不引入任何证书/加密代码，并在 `/AcroForm /SigFlags` 上保留既有位后置 bit 1。发现层把新 choice 值映射为 `selectedIndex`/`selectedIndices`/`multiSelect`，签名字段映射为 `signature`；对签名字段调用 `fill_form_field` 返回中文拒绝（“此字段仅创建签名结构，PDFX 不执行证书签名，无法填写。”）。选项标签允许重复或恰为 `Off`（choice 的 `/V` 是文本值，不是 on-state 名）；空标签清洗为“选项 N”；下拉只接受一个默认项，未启用多选的列表拒绝多个默认项。属性面板与创建工具新增“添加下拉框/添加列表框/添加签名字段”；签名字段只有名称/提示（无必填/字号），工具、面板与画布占位均显示中文披露“仅创建签名字段结构，不执行证书签名”，画布占位不渲染任何填写控件。创建、属性修改、删除、撤销/重做复用既有内容修改权限门禁与统一历史；重复保存不重复写字段，源字节前缀与 mtime 保持不变。

**自动化验证（合并 integration tip 后在 worktree 执行）：** `npm run typecheck` 通过；`npm test` **37 文件 / 401 项**全部通过（本票净增 5 项前端用例：下拉框属性、列表框多选、创建签名披露、发现签名只读占位、choice/signature 命令映射；其余为 integration 既有）；`npm run build` 通过；`npm run test:e2e` **12 项**通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 既有 4 条 warning）；`cargo test` **287 通过 / 0 失败 / 3 ignored**（`form_creation` 28 项，较 ticket 32 的 21 项净增 7 项；`form_fields` 10、`save_document` 22 及 integration 新增 `free_text` 13、`eraser` 8 等全部通过）。新增 Rust 用例：`creates_choice_fields_with_options_defaults_and_font_size`、`updates_choice_and_signature_properties_through_the_history`、`rejects_invalid_choice_and_signature_requests_without_creating_fields`、`choice_labels_may_be_duplicated_or_off_without_confusing_values`、`saves_choice_and_signature_structures_that_other_readers_agree_on`、`choice_and_signature_appearances_compensate_page_rotation`、`merges_choice_and_signature_into_an_existing_acroform_and_preserves_it`；权限用例扩展到六种请求。保存结构用例断言 `/Opt`/`/V`/`/I`、多选数组、空列表外观、签名无 `/V`/`/SV`/`/Lock`/`/AA`、BBox 等于 Rect、`SigFlags`、ToUnicode 含“中国签名”、唯一名称、重复保存不重复、PDFium 渲染有墨迹；旋转用例覆盖 0/90/180/270 的补偿矩阵与渲染。前端用例：`places a combo box and edits its options, default and font size`、`places a list box and edits multi-select defaults`、`places an empty signature field and discloses that PDFX does not sign`、`shows a discovered signature field as an inert placeholder`、`maps choice and signature requests to the shared generic seam`。

**人工未验项（未勾选）：** Preview、Adobe Acrobat Reader、Chrome 中列表/下拉/签名字段的显示与识别（真实阅读器矩阵）；真实 Tauri 窗口中的放置手感、中文输入法与属性面板焦点；加密且允许修改内容的源文档 + 创建 choice/签名字段 + Save As 组合。自动化只覆盖 PDFium 渲染/重开、独立 lopdf 结构与命令边界。

**已知限制/后续：** 不执行证书签名（本票范围）；画布上填写已创建 choice 字段只选择一个选项（多默认值需在属性面板设置），签名字段永远不可填写；不扁平化（ticket 35）；列表外观按选项数均分高度，不绘制滚动条。

