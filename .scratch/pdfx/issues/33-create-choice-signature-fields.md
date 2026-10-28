# 33: 创建列表/下拉与空签名字段

**What to build:** 用户可以创建列表/下拉 Form Field 与空签名字段；签名字段只建结构，不执行证书签名。保存后其他阅读器能识别字段。

**Blocked by:** 31 (创建文本字段)

**Status:** done

- [x] 列表/下拉字段可设置选项、默认值、多选（列表）与字号。
- [x] 空签名字段可放置并设置名称；保存后阅读器显示签名区域。
- [x] 不执行证书签名；UI 明确说明签名字段只提供结构。
- [ ] 保存后在 Preview、Adobe Acrobat Reader、Chrome 中字段结构可识别。（人工未验，见文末）
- [x] 撤销/重做覆盖创建、属性修改与删除。

**Verification (agent):** 分支 `ticket/33-create-choice-signature-fields`，功能提交 `ae27bfa`，合入 integration tip `0650973` 后为 `b8675e7`（基于 `03d8f56`；合并无冲突，`PDFX-PATCH.md` 与 integration tip 完全一致，本票没有新增 vendored hunk）。相对 integration tip 的代码改动为 9 个文件（+3315/−199）：`form_create.rs`、`form.rs`、`tests/form_creation.rs`、`tests/form_fields.rs`、`FormLayer.tsx`、`FormLayer.test.tsx`、`backend.ts`、`backend.test.ts`、`styles.css`；未改 spec、ADR、`Cargo.toml`/`Cargo.lock`/`package.json`/`package-lock.json`、`tauri.conf`、capabilities，未新增依赖。

**实现：** 列表/下拉复用 ticket 31/32 的 `CreatedFieldStore`、`FormFieldSpec`（新增 `Choice`/`Signature` 变体）、`ObjectEdit::CreateFormFields`、单一 writer `apply_created_fields`、`EditAction::FormStructure` 与单次 lopdf 增量 revision。choice 写 `terminal widget`：`/FT /Ch`、唯一 `/T`、`/Rect`、`/P`、`/F 4`、`/Ff`（Required bit 2；下拉 Combo bit 18；列表 MultiSelect bit 22，仅列表）、`/Opt`（UTF-16BE 文本串）、`/V`（单个选中写文本串，多选写文本数组，无默认省略）、`/I`（选中索引数组）、`/DA`（含字号）、`/Q 0`、`/Border`、显式 `/AP /N`；外观流用嵌入的 Noto Sans SC 子集绘制选中项（下拉含箭头与空态，列表逐行绘制选项并高亮选中项），字体同时进入 `/DR` 与 AP resources。签名字段写 `/FT /Sig`、唯一 `/T`、`/Rect`、`/P`、`/F 4`、`/Ff 0`、`/AP /N`（缓存标签“签名”、边框与签名线）与可选 `/TU`；刻意不写 `/V`、`/SV`、`/Lock`、`/AA`，不引入任何证书/加密代码，并在 `/AcroForm /SigFlags` 上保留既有位后置 bit 1。发现层把新 choice 值映射为 `selectedIndex`/`selectedIndices`/`multiSelect`，签名字段映射为 `signature`；对签名字段调用 `fill_form_field` 返回中文拒绝（“此字段仅创建签名结构，PDFX 不执行证书签名，无法填写。”）。选项标签允许重复或恰为 `Off`（choice 的 `/V` 是文本值，不是 on-state 名）；空标签清洗为“选项 N”；下拉只接受一个默认项，未启用多选的列表拒绝多个默认项。属性面板与创建工具新增“添加下拉框/添加列表框/添加签名字段”；签名字段只有名称/提示（无必填/字号），工具、面板与画布占位均显示中文披露“仅创建签名字段结构，不执行证书签名”，画布占位不渲染任何填写控件。创建、属性修改、删除、撤销/重做复用既有内容修改权限门禁与统一历史；重复保存不重复写字段，源字节前缀与 mtime 保持不变。

**自动化验证（合并 integration tip 后在 worktree 执行）：** `npm run typecheck` 通过；`npm test` **37 文件 / 401 项**全部通过（本票净增 5 项前端用例：下拉框属性、列表框多选、创建签名披露、发现签名只读占位、choice/signature 命令映射；其余为 integration 既有）；`npm run build` 通过；`npm run test:e2e` **12 项**通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 既有 4 条 warning）；`cargo test` **287 通过 / 0 失败 / 3 ignored**（`form_creation` 28 项，较 ticket 32 的 21 项净增 7 项；`form_fields` 10、`save_document` 22 及 integration 新增 `free_text` 13、`eraser` 8 等全部通过）。新增 Rust 用例：`creates_choice_fields_with_options_defaults_and_font_size`、`updates_choice_and_signature_properties_through_the_history`、`rejects_invalid_choice_and_signature_requests_without_creating_fields`、`choice_labels_may_be_duplicated_or_off_without_confusing_values`、`saves_choice_and_signature_structures_that_other_readers_agree_on`、`choice_and_signature_appearances_compensate_page_rotation`、`merges_choice_and_signature_into_an_existing_acroform_and_preserves_it`；权限用例扩展到六种请求。保存结构用例断言 `/Opt`/`/V`/`/I`、多选数组、空列表外观、签名无 `/V`/`/SV`/`/Lock`/`/AA`、BBox 等于 Rect、`SigFlags`、ToUnicode 含“中国签名”、唯一名称、重复保存不重复、PDFium 渲染有墨迹；旋转用例覆盖 0/90/180/270 的补偿矩阵与渲染。前端用例：`places a combo box and edits its options, default and font size`、`places a list box and edits multi-select defaults`、`places an empty signature field and discloses that PDFX does not sign`、`shows a discovered signature field as an inert placeholder`、`maps choice and signature requests to the shared generic seam`。

**人工未验项（未勾选）：** Preview、Adobe Acrobat Reader、Chrome 中列表/下拉/签名字段的显示与识别（真实阅读器矩阵，对应勾选项 4）；真实 Tauri 窗口中的放置手感、中文输入法与属性面板焦点。加密且允许修改内容的源文档 + 创建 choice/签名字段 + Save As 组合已由合并审计 probe 用 `encrypted-print-denied` fixture 覆盖（见下）；仍未覆盖的是“加密且已有 AcroForm 的源文档 + 新建 choice/签名 + 与既有字段合并保存”的组合。自动化只覆盖 PDFium 渲染/重开、独立 lopdf 结构、命令边界与该 probe。

**已知限制/后续：** 不执行证书签名（本票范围）；画布上填写已创建 choice 字段只选择一个选项（多默认值需在属性面板设置），签名字段永远不可填写；不扁平化（ticket 35）；列表外观按选项数均分高度，不绘制滚动条。

**合并与独立审计（merger，2026-10-29）：** 已独立复核实现、合并与门禁，未发现缺陷，因此**没有审计修复提交**；关闭提交为 `chore: close ticket 33`（本文件所在提交），同时把状态改为 done 并勾选 1/2/3/5。

- **合并**：worktree `/Users/nevs/Projects/pdfx-worktrees/33-create-choice-signature-fields` 先把 integration tip `eb327ff`（ticket 44 关闭提交）合入 `ticket/33-create-choice-signature-fields`，合并提交 `966dc3c`，无冲突；`git diff eb327ff..966dc3c` 仅本票 10 个文件（9 个代码文件 + 本票据记录），`git diff eba6b3e..966dc3c` 仅 ticket 44 的 11 个文件，两侧内容完整保留。集成树 `/Users/nevs/Projects/pdfx` 以 `git merge --no-ff ticket/33-create-choice-signature-fields` 合入，合并提交 **`73a581a`**（父 `eb327ff` + 分支，无冲突）；`git diff --stat eb327ff..73a581a` 为 10 文件（+3326/−199）。相对 integration tip 未改 `src-tauri/vendor`（`PDFX-PATCH.md` 仍为 15 文件/10 类型，`git diff` 为空，无新 hunk）、`Cargo.toml`/`Cargo.lock`/`package.json`/`package-lock.json`、spec、ADR、`tauri.conf` 或 capabilities，未新增依赖/网络/遥测。

- **勾选 1（choice 属性与保存结构）**：写入 `/FT /Ch` 终态 widget；`/Ff` 仅写 Required `1<<1`、下拉 Combo `1<<17`（bit 18）、列表 MultiSelect `1<<21`（bit 22）。MultiSelect 只可能出现在列表：ComboBox 的创建与更新请求结构体都没有 multiSelect 字段，列表则显式接收；Edit `1<<18` 从不设置；Required 在属性更新中保留。`/Opt` 恒非空（空列表→“选项 1”，空/空白标签→“选项 N”），重复标签与字面 “Off” 原样保存并靠 `/I` 区分；单选写文本 `/V`，多选写与 `/I` 同序的文本数组，无选择时省略 `/V` 与 `/I`；越界默认项、未开多选时的多个默认项均以中文拒绝，删除选项后旧索引同样被拒且字段不变（probe）。`/DA` 含字号（形如 `"/PdfxNoto 12 Tf 0 g"`）、`/Q 0`，AP `/N` 用同一字体与字号绘制选中项（下拉含箭头与空态，列表逐行绘制并高亮）；CJK 经嵌入的 Noto Sans SC 子集渲染，ToUnicode 覆盖“中国签名”“甲/丁/Off”等。属性编辑、撤销/重做、删除、重复保存后 `/Opt`/`/V`/`/I` 均由 session 状态重新推导，保持一致；probe 还验证 300 字符长标签与空标签原样/稳定保存。用例：`creates_choice_fields_with_options_defaults_and_font_size`、`choice_labels_may_be_duplicated_or_off_without_confusing_values`、`updates_choice_and_signature_properties_through_the_history`、`rejects_invalid_choice_and_signature_requests_without_creating_fields`。

- **/T、发现、填写、权限与删除**：新字段名经 `validate_name`（非空、无控制字符、≤128 字符），与全部既有字段及本会话字段精确比对（PDF 名称大小写敏感，故大小写不同的名称是合法独立字段），自动名 `combo_box_N`/`list_box_N`/`signature_N` 同时避开两侧已用名称。`/T` 是真实名称，`new:<uid>` 只存在于会话 field_id，保存前重新生成/校验，绝不写入 PDF（probe 保存后读取为 `list_box_1`/`encrypted_country` 等）。发现层按页优先立即包含 choice（`selectedIndex`/`selectedIndices`/`multiSelect`）与 signature；probe 确认新列表紧跟第 1 页既有 widget 的 tab 顺序。画布填写已创建 choice 会写 `/V`+`/I`（probe：`/V "C"`、`/I [2]`）并刷新面板/AP；无内容修改权限时创建（六种类型）、更新、删除与 `FormStructure` 撤销/重做都经同一内容权限门禁返回中文错误（代码核对：`ensure_creatable` / `ensure_content_editable`），权限用例对六种创建请求断言 `PermissionDenied`、消息含“权限”且历史保持 revision 0。删除从会话移除并在保存后不再出现于 `/Fields`/`/Annots`（历史用例 + SigFlags probe）。用例：`form_fields` 新增 `selected_indices`/`multi_select` 断言；`updates_choice_and_signature_properties_through_the_history`；权限测试。

- **撤销/重做与保存边界**：创建、每次属性修改、填写、删除都压入单个 `EditAction::FormStructure` 快照；撤销/重做需内容权限，恢复快照后重新生成发现视图。保存仍只有 ticket 10 管线：源文件字节与 mtime 不变（干净且无输出路径时直接返回源字节），每次保存都从源 PDF 重新生成字段，重复保存不累积 `/Fields`/`/Annots`/字体（probe：两次保存后字体 1、字段 2、注释 2），并保持源 `/NeedAppearances`、AcroForm 既有项、DR/DA 与既有值。用例：`repeated_saves_never_duplicate_created_fields`、`merges_choice_and_signature_into_an_existing_acroform_and_preserves_it`、`save_document` 全 22 项。

- **签名结构**：`/FT /Sig`、唯一 `/T`、可选 `/TU`、`/Rect`、`/P`、`/F 4`、显式 `/AP /N`（白底、边框、“签名”文字与签名线，含旋转补偿），`Ff 0`；刻意不写 `/V`、`/SV`、`/Lock`、`/AA`，无证书/加密/签名代码，也不写 `/NeedAppearances`。发现层映射为 signature；`fill_form_field` 返回中文“此字段仅创建签名结构，PDFX 不执行证书签名，无法填写。”；`required_missing` 不把签名当必填，故不会阻塞其他字段保存。UI 工具、属性面板与画布占位均显示“仅创建签名字段结构，不执行证书签名”；已创建签名占位可选中但无任何填写控件，发现的签名是 `role="img"` 惰性占位；重命名/删除/撤销/重做走同一历史。测试：`places an empty signature field and discloses that PDFX does not sign`、`shows a discovered signature field as an inert placeholder`、保存结构用例对 `/V`/`/SV`/`/Lock`/`/AA` 的缺失断言、`updates_choice_and_signature_properties_through_the_history`。

- **SigFlags**：仅在有新建签名字段时写 `existing | 1`（SignaturesExist），从不写 AppendOnly `2`，并保留外来位；probe 用 `/SigFlags 4` 的合成 PDF 验证新增签名后为 5、撤销删除并保存后回到 4（bit 1 不残留），已有签名的文档不会被本票宣称存在新签名。

- **独立阅读器结构与加密源**：保存用例先用独立 lopdf 解析（`/FT`、`/Opt`、`/V`、`/I`、`Ff`、`/DA`、`/Q`、`/AP /N`、BBox=Rect、SigFlags、ToUnicode），再以新的 PDFium 例程重开并渲染、断言有墨迹；旋转用例覆盖 0/90/180/270 的补偿矩阵与渲染。probe 用 `encrypted-print-denied` fixture 创建 choice+签名并 Save As（确认输出不加密），输出含 `encrypted_country`/`encrypted_sign` 且 SigFlags bit 1 置位。

- **审计 probe（临时、未提交）**：上述 probe 均以临时集成测试运行后删除，只保留结果记录；本关闭提交只改本票据文件。

- **门禁（集成树 `73a581a`，2026-10-29）**：`npm run typecheck` 通过；`npm test` **37 文件 / 418 项**通过（较 ticket 44 关闭后的 413 项净增 5 项本票前端用例）；`npm run build` 通过；`npm run test:e2e` **12 项**通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 既有 4 条 warning，未用 `--all-targets`）；`cargo test` **288 通过 / 0 失败 / 3 ignored（29 个 suite，含 doc-tests）**；其中聚焦 `form_creation` 28、`form_fields` 10、`save_document` 22 全过；较 integration tip 的 281 项净增 7 项本票 Rust 用例。

- **限制/后续**：不执行证书签名；画布填写已创建 choice 只能单选（多默认值在属性面板设置），签名字段永远不可填写；不扁平化（ticket 35）；列表外观按选项数均分高度、长标签不换行也不截断（probe 300 字符标签原样保存，显示可能溢出）；真实阅读器矩阵与真实 Tauri 交互仍为人工未验项。

**分支/工作树：** `ticket/33-create-choice-signature-fields` 保留在 `966dc3c`；工作树 `/Users/nevs/Projects/pdfx-worktrees/33-create-choice-signature-fields` 已 `cargo clean`（释放 6.6 GiB）并移除；集成树干净，磁盘可用 16 GiB。

