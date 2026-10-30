# 32: 创建复选框与单选按钮

**What to build:** 用户可以创建复选框与单选按钮组，设置名称、默认状态与选项，保存后其他阅读器能识别并填写。

**Blocked by:** 31 (创建文本字段)

**Status:** done

- [x] 复选框可设置默认选中/未选中，保存后 `/Yes`/`/Off` 状态正确。
- [x] 单选按钮组可添加多个选项，`/Kids`、`/AS` 与导出值正确。
- [ ] 在 Preview、Adobe Acrobat Reader、Chrome 中可切换状态并保存。（结构、外观与 PDFium 重开已由自动化和独立 lopdf 解析验证；三个阅读器未人工打开验证，保持未勾选，见文末人工矩阵）
- [x] 撤销/重做覆盖创建、选项编辑与删除。

## Verification notes

**Merge:** 分支 `ticket/32-create-checkbox-radio` tip `7ddf10d`（feature `34aac67`；分支此前已把 integration tip `0066352` 合入 `7ddf10d`，`git merge-base` 即 `0066352`）。在 integration 执行 `588e48b`（`merge: ticket 32 checkboxes and radio groups`，9 文件，+3758/−450），git 无冲突、无需手工解决。合并后审计发现两个边界问题，在 `943e643`（`fix: keep radio on-states and property panel state consistent`，4 文件，+203/−5）修复并加回归测试；该修复只改 `form_create.rs`/`form_creation.rs`/`FormLayer.tsx`/`FormLayer.test.tsx`。相对 `0066352` 未改 spec、ADR、`Cargo.toml`/`Cargo.lock`/`package.json`/`package-lock.json`、打包配置或 vendored patch；本票未新增 vendored hunk（`PDFX-PATCH.md` 计数不变）。

**审计发现并修复：**
1. **`/Off` 导出值碰撞（后端）**：选项标签若清洗后恰为 `Off`，原先直接成为 on-state，导致 `/AP /N` 的 `Off` 键被 on 外观覆盖、父 `/V` 与 kid `/AS` 都变成 `/Off`，保存重开后该选项被 PDFium 视为未选。修复：`radio_export_value` 保留 `Off`，此类标签回退为 `ChoiceN`（仍参与组内去重）。新增 `radio_options_never_use_the_reserved_off_state`：选项 `["Off","On"]` 默认第 1 项，会话内导出值为 `["Choice1","On"]`，保存后父 `/V /Choice1`、kid `/AS` 为 `Choice1`/`Off`、`/AP /N` 同时含 `Off` 与 `Choice1` 且外观不同，重开后仍选中第 1 项、必填门禁为空。
2. **属性面板草稿陈旧（前端）**：在画布上直接勾选/选择本次会话创建的复选框或单选后，属性面板的 `checked`/`selectedIndex` 草稿未刷新；随后改名称、提示、必填或增删选项时会以旧值提交 `update_form_field`，静默把刚填的状态改回去。修复：填写成功且面板正编辑同一字段（按创建字段基 id 比较）时，只同步 `checked`/`selectedIndex`，不丢弃其他未提交草稿。新增 `FormLayer.test.tsx` 用例：画布勾选后“默认选中”变为选中，再改名称提交的 `checked` 为 `true`。

**复选框结构：** `write_checkbox` 写 terminal field + widget：`/FT /Btn`、`/Subtype /Widget`、唯一 `/T`、`/Rect`、`/P` 指向页面、`/F 4`、`/Ff`（必填时 bit 2）、`/V` 与 `/AS` 同为 `/Yes` 或 `/Off`、显式 `/AP /N` 字典同时含 `Off` 与 `Yes` 两个 Form XObject（内容不同，勾选态绘制对勾）、`/Border [0 0 0]`、可选 `/TU`；不写 `/NeedAppearances`。默认选中/未选中由创建请求和属性面板的“默认选中”控制；`updates_checkbox_properties_through_the_history_and_rejects_wrong_values` 断言未选中保存后 `/V`/`/AS` 均为 `Off`、必填 bit 与中文 `/TU`；`saves_checkbox_and_radio_widget_structures_that_other_readers_agree_on` 断言选中保存后 `/V`/`/AS` 为 `Yes`、`/AP` 两态存在且像素不同、`/Fields` 中复选框不是 radio（bit 16 为 0）。新建复选框通过 `discover_all` 立即出现，可由 ticket 30 的 `fill_form_field` 勾选/取消（`creates_checkboxes_that_discovery_reports_and_the_shared_history_toggles`），必填门禁在未勾选时报告、勾选后清空、undo/redo 往返；填写使用表单填写权限，创建/改属性/删除使用内容修改权限。

**单选组结构：** `write_radio_group` 写一个父字段 + 每个选项一个 kid：父字段 `/FT /Btn`、`/Ff` Radio bit（bit 16）+ 必填 bit、`/T`、`/Kids`、`/V`、可选 `/TU`，没有 `/Rect`/`/AP`；每个 kid 有 `/Parent`、`/Rect`、`/P`、`/F 4`、`/AS`、`/AP /N`（`Off` 与各自 on-state，绘制圆点）和 `/Border`，不重复 `/T`。父 `/V` 与恰好一个 kid 的 `/AS` 由同一 `selected` 索引派生，未选时全为 `/Off`。导出值由选项标签派生（ASCII 字母数字；无 ASCII 时 `ChoiceN`；`Off` 保留；组内重名加 `_2` 等后缀），因此中文标签、重复标签和 `Off` 标签都得到合法且互不相同的 on-state。选项增删改重命名走 `update_form_field` 的整组重建（`updates_radio_options_and_default_together_and_history_restores_them` 覆盖新增选项并设默认、改标签并删选项、非法默认/重复名/空白名被拒且字段与历史不变；`FormLayer.test.tsx` 覆盖面板添加/删除/改标签/设默认及删除后索引修正）。必填组只报告一次（组首 widget），默认选中后门禁清空、undo 后重新出现（`creates_radio_groups_with_kid_widgets_and_a_consistent_value`）。删除组内任意 kid id 都删除整组并可 undo 恢复；重复保存后 `/Fields` 每个字段一次、`/Kids` 不重复、页面 `/Annots` 不出现孤儿 widget（`repeated_saves_never_duplicate_created_fields` 的文本路径 + `saves_checkbox_and_radio_widget_structures...` 的第二次保存断言；保存每次从 Source 重新派生，会话记录不清空，因此保存后继续改选项/删除会反映到下一次输出）。

**发现/顺序/覆盖层：** 新字段在创建命令返回的 `fields` 中立即出现；`merge_views` 按页序合并，同页已有 widget 在前、会话创建字段按 uid 和选项序在后，并重编连续 `order`，Tab 顺序与 `missing_required_fields` 直接复用该列表。会话 id `new:<uid>[:<option>]` 只存在于内存；`saves_checkbox_and_radio...` 与 `merges_checkbox_and_radio...` 均断言输出字节中不存在 `new:`。`/Fields` 只新增父字段/terminal field，radio kids 只进页面 `/Annots`；已有 `/Fields` 条目、顺序与值保留，新 `/Annots` 追加在已有注解之后（merge 测试逐项断言）。混合创建文本框 + 复选框 + 单选组的审计探针（已删除，未提交）在合成页上确认：`/Fields` 3 项、页面 `/Annots` 4 项（text widget、checkbox、两个 radio kid）、发现顺序 0–3、必填门禁只列未填文本与复选框各一次；`/DR` 只嵌入文本字段所需的 `PdfxNoto`，`/DA` 写为新字体，`/NeedAppearances` 不存在；输出比源多 2 个 `startxref`（PDFium 一次增量 + lopdf 至多一次 revision），源字节与 mtime 未变。

**保存与保真：** 本票没有独立保存路径，创建字段作为单个 `ObjectEdit::CreateFormFields { fields }` 与既有表单对象编辑一起进入 ticket 10 的 `save_document_with_progress`，仍只调用一次 `append_object_edits`（一个 lopdf revision）且派生自 PDFium 增量输出。`merges_checkbox_and_radio_into_an_existing_acroform_and_preserves_it` 在 `form-acroform.pdf` 上确认：已有字段值（`full_name`、`country`）、源 `/DA` `/Helv 0 Tf 0 g`、源 `/DR /Font /Helv` 与源 `/NeedAppearances true` 原样保留；只有复选框/单选时不再嵌入 `PdfxNoto`；`/Fields` = 源条目 +2（复选框 + radio 父），页面 `/Annots` = 源 +3（复选框 + 两个 kid）且已有注解顺序不变。文本框场景沿用 ticket 31 的字体子集条件（存在 `FormFieldSpec::Text` 才读/写 `/DR`、按需写 `/DA`）；`merges_created_fields_into_an_existing_acroform` 在合并后仍通过。代码从不写 `/AA`、`/K`、`/JS` 或任何会执行的动作；无 `/NeedAppearances` 写入路径。保存后输出以源字节为前缀（`starts_with` 断言），Source Document 永不修改。

**撤销/重做与权限：** 创建、复选框默认状态、单选选项增删改、默认选项、删除、对创建字段的填写全部是一个 `EditAction::FormStructure`，携带整条 `CreatedField` 快照（含 widgets/导出值/几何/选项/值），经 ticket 31 的统一 `undo_stack`/`redo_stack` 恢复；`creates_checkboxes...`、`updates_radio_options...`、`updates_checkbox...`、`creates_radio_groups...`、`fills_a_created_field...` 覆盖往返，保存不切断历史。权限：创建/更新/删除入口要求内容修改权限（`ensure_content_editable`，中文“此文档的权限限制不允许创建表单字段。”），undo/redo 的 `FormStructure` 分支同样要求内容权限；`permission_restricted_documents_refuse_creation_with_a_chinese_reason` 对文本框、复选框、单选三种请求断言 `permission_denied`、中文原因、字段列表为空、revision 0 且不 dirty。前端在 `canModifyContent == false` 时禁用三个创建按钮并显示同一中文原因（`FormLayer.test.tsx`）。90/180/270 旋转页：`checkbox_and_radio_appearances_compensate_page_rotation` 断言复选框与 radio kid 的 `/Rect`、`/BBox`、补偿矩阵（`0 1 -1 0 …`、`-1 0 0 -1 …`、`0 -1 1 0 …`）、父 `/V`/kid `/AS`，并用 PDFium 渲染像素确认勾选与圆点横向直立、重开后几何回到绘制矩形。

**前端/中文/隐私/依赖/seam/范围：** “表单工具”新增三个按钮“添加文本字段/添加复选框/添加单选按钮组”（互斥的放置模式；按钮默认 16×16 pt，文本 180×24 pt；点击或拖动放置，最小 4 pt 后端校验、页面内校验）。属性面板按类型显示：文本的名称/提示/必填/多行/字号，复选框的名称/提示/必填/默认选中，单选的名称/提示/必填/选项列表（增删、行内编辑、Enter/blur 提交、Escape 还原）与默认选项；单选组所有 option widget 在面板中折叠为一个条目，画布上每个选项仍独立渲染。新增 UI 与错误提示均为中文；后端复用 `form_field_name_invalid`/`form_field_name_duplicate` 中文消息及新中文校验（选项越界、默认无效、单选不能取消等）。代码只使用本地 PDFium/lopdf/既有字体子集，无网络、遥测、上传或字段值日志；相对 `0066352` 无新增 Rust/npm 依赖。能力仅经 `create_form_field`/`update_form_field`/`delete_form_field` 暴露，前端只经 `src/lib/backend.ts` 调用；ticket 31 的 `create_form_text_field`/`update_form_text_field`/`delete_form_text_field` 命令保留作兼容 seam，前端创建流程已改走通用命令。跨工具刷新仍只经 `documentEvents` 的 `source: "form"`；未实现 ticket 33 的列表/下拉/签名字段（`CreateFormFieldRequest` 只有 text/checkbox/radio 三个变体）。

**自动化验证（合并 + 修复后在 integration 树执行）：** `npm run typecheck` 通过；`npm test` **34 文件 / 354 项**全部通过（较 `0066352` 基线净增 4 项：`FormLayer.test.tsx` 复选框、单选、面板同步 3 项，`backend.test.ts` 命令映射 1 项；其中面板同步为审计修复新增）；`npm run build` 通过；`npm run test:e2e` **12 项**通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 既有 4 条 warning）；`cargo test` **259 通过 / 0 失败 / 3 ignored**（`form_creation` 21 项，较 ticket 31 的 11 项净增 10 项：本票 9 项功能用例 + 1 项 `/Off` 回归；`form_fields` 10、`save_document` 22、`text_edit` 18、`stamps` 9、`note_ink` 16、`shapes` 8、`lines` 8、`image_insertion` 13、`text_boxes` 11 全部通过，无回归）。`fixtures/generate.py` 与 fixture 未修改，未运行 fixture 再生成。审计临时探针（`/Off` 复现、混合字段保存）均已删除，不提交。

**人工未验项：** Preview、Adobe Acrobat Reader、Chrome 中复选框/单选组的显示、切换与保存（勾选 3 保持未勾选）；真实 Tauri 窗口中的放置手感、中文输入法、面板焦点与 Tab 跨页；加密源 + 创建字段 + Save As 确认路径（沿用 ticket 31 已知缺口，现有加密 fixture 无法构造“允许改内容 + 创建字段”组合）。自动化仅覆盖 PDFium 渲染/重开、独立 lopdf 结构与命令边界。

**已知限制/带入 ticket 33：**
- 单选选项的**显示标签不持久化**：PDF 结构只保存 kid 的 on-state（本票按标签清洗后派生，中文/纯符号回退 `ChoiceN`），重开后 PDFX 与阅读器看到的是导出值而非原始标签（重开后的 radio 视图 `options` 为空，标签退化为 `name（exportValue）`）。若产品要求标签跨会话保留，需要在父字段写 `/Opt` 数组并在发现层读取；ticket 33 做 choice 字段时一并补 `/Opt`/`/V`/`/I` 时需区分 choice 与 radio 的 `/Opt` 语义。
- ticket 33 的列表/下拉必须写 `/Opt`、`/V` 与 `/I`，并让选中值外观进入同一字体子集管线；choice 的 `/DA`/`/DR` 与外观流应复用本票的字体嵌入条件（仅当存在需要文本外观的字段时嵌入）。
- ticket 33 的签名字段只建空结构：需要 `/FT /Sig`、widget `/Rect`/`/P`/`/F` 与显式 `/AP /N`（或按 ticket 29 建议补 `/MK` 边框），不写 `/V`、不执行证书签名；继续使用内容修改权限，并出现在 `discover_all` 同一列表、同一 `order` 中。
- 复用本票/ ticket 31 的 `CreatedFieldStore`、`FormFieldSpec`、`EditAction::FormStructure`、`merge_views`、`ObjectEdit::CreateFormFields` 与单一 lopdf revision；新类型必须同时写 `/Fields`、页面 `/Annots` 与（如需要）`/DR`，并保证重复保存不重复写对象、undo/redo 与删除能反映到下一次保存。
- 名称唯一性检查继续同时覆盖已有 PDFium widget 与会话创建字段；选项/导出值去重需继续保留 `Off` 等保留状态。
- 人工矩阵（Preview/Acrobat/Chrome、加密源组合、真实 Tauri GUI）随 32/33 一并补齐；建议补一张“加密且允许修改内容 + 已有表单”的 fixture。
