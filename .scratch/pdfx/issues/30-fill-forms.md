# 30: 填写已有表单 + 必填校验

**What to build:** 用户可以在 M3 识别并填写已有 Form Field（文本、复选框、单选、列表/下拉），必填字段在保存前提示；保存后其他阅读器能识别填写结果。

**Blocked by:** 10 (Save As / Save Changes 管线)

**Status:** done

- [x] 自动识别已有 Form Field 并在画布上高亮/可点击。
- [x] 文本、复选框、单选、列表/下拉可填写；Tab 在字段间移动。
- [x] 必填字段为空时保存前提示，并定位到第一个未填字段。
- [ ] 保存后在 Preview、Adobe Acrobat Reader、Chrome 中显示填写结果。
- [x] 不执行计算或 JavaScript 动作。

**Verification notes:**

- 合并提交 `f82a69e`（`git merge --no-ff ticket/30-fill-forms`）；分支合并提交 `634ef2c`（把 integration tip `51390fd` 合入分支 tip `a7f3edc`，feature `bc699df`）。分支侧基线已含 ticket 11（`9f38ee0`；`a7f3edc` 另合入 `c0e09c3`），integration 侧另含 ticket 14（图片）与 ticket 11。合并基为 `9f38ee0`。合并后未修改 spec、ADR、打包配置或其他 ticket；关闭提交只改本文件。
- 冲突与解决（6 个文件）：
  - `src-tauri/src/text_edit.rs`：`EditAction` 合并为 `Box`/`Image`/`Field`/`Annotation` 四支；`undo_for`/`redo_for` 保留 integration “先看栈顶再弹出”的权限分派，并按动作分派：Box/Image → `can_modify_document_content`，Field → `can_fill_existing_interactive_form_fields`，Annotation → 注释权限；保留 ticket 30 的 `undo_field`/`redo_field`，Box 与 Image 路径继续共用 `shift_object_indexes`。
  - `src-tauri/src/lib.rs`：同时保留 `mod form`/`image_edit` 与全部表单、图片命令注册。
  - `src/lib/backend.ts`：图片视图/请求类型与表单类型并集，删掉交错重复的类型块。
  - `src/lib/documentEvents.ts`：ticket 30 的 `origin?: "form" | "text"` 与 integration 的 `source?: "text" | "image"` 统一为 `source?: "text" | "image" | "form"`；同步改 `FormLayer.tsx`、`PageTextLayer.tsx` 与两处测试，并删除 `PageTextLayer` 中重复的图片专用历史刷新（统一由 `source !== "text"` 分支处理）。
  - `src/components/PageViewer.tsx`：同一 `PageSurface` 上先渲染 `PageImageLayer`，再渲染 `FormLayer`，两者接收同一显示空间宽高与 `readOnly` 派生权限。
  - `src/App.test.tsx`：mock 命令表同时保留表单与图片命令。
  - 自动合并正确：`src-tauri/vendor/pdfium-render/PDFX-PATCH.md` 合并为 “Ten files change”，即 ticket 11 的九个文件加 ticket 30 的 `radio.rs` `export_value()` hunk（已逐文件核对计数与 hunk）；fixtures、`App.tsx`、`useDocumentSave.ts` 均保留双方改动。语义冲突修复：`PageImageLayer` 不再因自己的 `source === "image"` 事件从可能滞后的服务端列表重载而覆盖刚提交的变换；插入按 `imageId` 去重。该修复同时解决了 integration 基线 `51390fd` 上已存在的 2 项 `PageImageLayer` 测试失败（resize/rotate 后回读旧旋转值）。
- 识别/几何/顺序：`form_fields` 按页序、再按每页 annotation 顺序发现，返回显示空间矩形（`DisplayGeometry`，含 CropBox 与 `/Rotate`）、`required`、`readOnly`、`multiline`、`options`、`selectedIndex`、单选组名与导出值。`discovers_every_existing_field_in_page_and_widget_order` 断言两页 12 个 widget 的名称/类型/页码与严格递增的 Tab `order`，以及 `full_name` 在 612×792 页面上的显示矩形（x=72、y=148、248×24）与 `notes` 的多行标记、`readonly_note` 的只读标记；`missing_required_fields` 对单选组只报一次（每组首 widget），只读字段永不计入缺失。前端 `FormLayer` 用百分比定位覆盖层并在同一页渲染真实 `<input>/<select>/<textarea>`，`FormLayer.test.tsx` 覆盖渲染、点击/选择提交与中文标题；`App.test.tsx` 覆盖受限文档全部控件禁用并显示“此文档不允许填写表单”。
- 填写：文本（含多行，Enter 提交、多行内换行不提交、Escape 撤销草稿、blur/Tab 提交）、复选框、单选组、列表/下拉均经 `fill_form_field` 命令。单选取值写在父字段 `/V`、每个 kid 的 `/AS` 同步；choice 写 `/V` 与 `/I`（来自 `/Opt`）。`fills_a_text_field_updates_required_state_and_marks_the_document_dirty`、`toggles_checkboxes_on_and_off_through_history`、`selects_one_radio_widget_per_group_and_reports_the_group_as_answered`、`selects_list_and_dropdown_values_from_their_options`、`saves_filled_values_that_independent_readers_can_reopen` 覆盖全部类型；非法 choice 返回 `FormValueInvalid` 且不修改文档（`selects_list_and_dropdown_values_from_their_options`）。Tab 在字段间移动：`FormLayer.test.tsx` 断言文本提交后焦点到下一字段，以及页内最后一个可编辑 widget 按 Tab 会调用 `onSelectPage(1)`/`onFocusField("1:1")` 跨页导航；只读字段被跳过。
- 必填门禁：`useDocumentSave.checkRequired` 在 `saveAs` 选目标之前、`saveChanges` 写盘之前调用 `missing_required_fields`；缺失时显示中文横幅“有 N 个必填字段尚未填写，无法保存”、列出最多 3 个“第 X 页「字段名」”、提供“定位”按钮并自动跳到第一缺失字段所在页且聚焦该 widget；字段补填后横幅随 `documentEvents` 自动更新/消失。`App.test.tsx` 的 “blocks saving while required fields are empty and locates the first one” 断言两个按钮都被拦截、`save_document` 与保存对话框从未调用、页面导航到第 2 页且 `notes` 获得焦点。原生菜单加速器（Cmd/Ctrl+S、Cmd/Ctrl+Shift+S）只产生 `pdfx://menu` 的 `file.save`/`file.save_as`，`App.handleMenu` 调用同一 `saveChanges`/`saveAs`，不存在第二条保存路径（`App.test.tsx` 覆盖菜单 id 接线）；后端 `save_document` 本身不重复校验必填（门禁只在 UI 保存入口），拖拽/其他 IPC 直调不在产品路径内。
- 不执行 JS/计算：填写只经 PDFium 的注解值 API 与 lopdf 对象写入，从不执行 `/AA`、`/K`、计算脚本。`js_guard` fixture 带 `/AA /K` JavaScript keystroke 动作，`a_java_script_keystroke_action_is_never_executed` 断言输入值原样保留、清空仍为空；保存后用独立 lopdf 解析确认 `/AA /K/JS` 脚本字节原样保留。全仓库无写入 `/NeedAppearances` 的代码（grep 无匹配），保存不做表单扁平化、不重算字段、不重建 AcroForm 字典。
- 保存管线与写入者：表单值全部走 ticket 10 的 `save_document_with_progress`：`serialize_output` 一次 PDFium 增量（或加密源 `FPDF_REMOVE_SECURITY` 重写），随后 `append_object_edits` 至多追加一个 lopdf 增量（单选组 `/V`+kids `/AS`、choice `/V`+`/I`；无对象级表单编辑时不追加）。审计临时探针（已删除，不提交）实测：仅填文本字段输出比源多 1 个 `startxref`（PDFium）；填文本+单选+下拉多 2 个（一次 PDFium + 一次 lopdf）。`saves_filled_values_that_independent_readers_can_reopen` 断言 Source Document 字节与 mtime 均未变、输出以源字节为前缀；通过命令边界重开输出后 PDFium 读到文本/复选框/单选选中项/list+combo 值与 `selectedIndex`、必填列表为空；独立 lopdf 解析确认父字段 `/V female`、kids `/AS [Off, female, Off]`、`country /V Germany /I [2]`、`agree /AS /Yes`，且 `/AA /K` 脚本未被改写。
- 撤销/重做与索引：Field 动作进入与 Box/Image/Annotation 相同的 `EditSession` 历史，保存前后的字段值快照由 `apply_field_snapshot` 恢复。`undo_and_redo_restore_values_through_the_shared_edit_history` 断言单选与文本的往返及 revision 计数；`read_only_fields_and_restricted_documents_refuse_filling` 断言被拒绝的填写不产生历史。审计临时探针在同一文档依次执行 Text Box → Field 填写 → 插入图片 → 创建高亮注释，历史 revision=4，按最新优先撤销时注释/图片/字段/文本框依次回退（字段恢复为空值，各工具状态互不泄漏），再全部重做回到 revision=4；权限分派为代码级核对（`undo_for`/`redo_for` 对栈顶动作分派三种权限检查）。保存不切断历史由 ticket 10 的通用保存测试（Text Box 动作）与 ticket 14 的 `undo_after_save_removes_the_image_from_a_later_save` 覆盖，Field 动作共用同一 `saved_revision`/`history_view` 机制；未单独写“保存后撤销表单填写”的用例（已知小缺口）。表单是注解/widget 对象，不移动页面内容对象索引，因此 Field 动作不调用 `shift_object_indexes`；图片/Text Box 的索引同步测试在合并后全部通过（`image_actions_keep_text_box_object_ranges_in_sync` 等）。
- 权限/只读/加密：只读字段返回 `FormFieldReadOnly` 且不 dirty；`encrypted-owner-restrictions` 文档 `permissions.can_fill_forms == false`，填写返回 `FormFillNotPermitted` 且不修改文档（`read_only_fields_and_restricted_documents_refuse_filling`）。前端对 `canFillForms` 为 false 的文档禁用全部控件并显示中文原因。加密源的保存要求 `confirm_unencrypted_output`（ticket 10/ADR-0007 路径，弹确认后才写无密码输出）；当前 fixture 中没有“加密且含可填表单”的组合，未单独走“加密源 + 填表 + Save As 确认”的端到端测试，作为人工/后续项。
- 隐私/依赖/文案/单 seam：相对合并基无 `package.json`/`Cargo.toml`/lock 变化（无新 crate/包、无新许可证）；新增代码无网络、遥测、上传或日志写入字段值；错误信息在后端为英文（沿用命令边界约定），前端按错误码映射中文，新增 UI 文案均为中文。表单能力仅通过 3 个 Tauri 命令 `form_fields`、`missing_required_fields`、`fill_form_field` 暴露，前端只经 `src/lib/backend.ts` 调用；跨工具刷新只经 `documentEvents` 的 `source: "form"` 事件。唯一 vendored 改动是 `radio.rs` 的 `export_value()`（补 PDFium `FPDFAnnot_GetFormFieldExportValue()` 在固定版本返回空的缺口，另有 lopdf 回退），属于既有 MIT/Apache-2.0 vendored crate 的增量 hunk，不引入新依赖。
- 自动化验证（合并后在仓库根执行）：`npm run typecheck` 通过；`npm test` 26 文件 262 项全部通过；`npm run build` 通过；`npm run test:e2e` 12 项通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 既有 warning，符合预期）；`cargo test` 190 项通过、0 失败、3 项 ignored（既有手动/延迟测试），其中 `form_fields` 10 项、`image_insertion` 13 项、`save_document` 22 项、`text_edit` 18 项；`python3 fixtures/generate.py --check` 输出 `fixtures OK (11 PDFs)`；`swift fixtures/verify.swift` 全部通过（含 2 页 12 widget 的 form-acroform.pdf）。
- 勾选 4 **未验证（人工）**：自动化替代证据充分——保存后通过命令边界重开，PDFium 能读出全部填写值，独立 lopdf 能解析 `/V`、单选 `/AS`、choice `/I`，证明结构与值正确；但 Preview、Adobe Acrobat Reader、Chrome 中的实际显示（尤其文本字段的外观流是否被各阅读器采用、是否需要 `/NeedAppearances`）未在本机手动打开验证，故保持未勾选，留给跨阅读器人工矩阵。注意：文本字段只写 `/V`，依赖 PDFium 的字段值 API；若某阅读器显示旧外观，需在人工矩阵中确认并考虑写入显式 `/AP`。
- 已知限制/后续（ticket 31–33）：
  - 创建字段必须复用 `form.rs` 的发现/`FormFieldView`/`EditAction::Field` 模型；新字段要同时写 `/AP` 外观流与 `/DA`/`/DR` 字体资源，否则其他阅读器可能显示空框。ticket 30 只填已有字段，不创建 AcroForm 字典。
  - 权限分类：填写用 `can_fill_existing_interactive_form_fields`；创建/删除字段是结构修改，应使用内容修改权限（`can_modify_document_content`）并在 `undo_for`/`redo_for` 增加对应分支，不要复用 Field 的填写权限。
  - 单选/choice 的对象级值由 `FormOverlay` 从撤销栈推导，创建/删除字段后需保证 overlay 与实际 AcroForm 一致；ticket 32 创建单选组时必须写父 `/V` 与每个 kid 的 `/AS`，并继续使用 `radio.rs` 的 `export_value()` 读取 on-state。
  - 必填门禁在每次保存前调用 `missing_required_fields`；新字段创建后要出现在 `form_fields` 的结果中，否则门禁与 Tab 顺序看不到它们。`EditAction` 联合新增动作时保持单一历史与按动作权限分派；新字段动作若增删页面内容对象需维护 `shift_object_indexes`（表单 widget 本身不移动内容对象）。
  - 跨阅读器显示矩阵（Preview/Acrobat/Chrome）仍为人工项，ticket 31–33 的“保存后其他阅读器能识别”应一并纳入；建议补一张“加密且含表单”的 fixture 与旋转页面表单 fixture，覆盖加密 Save As 确认路径与显示几何。
  - 未验证的人工 GUI 项：真实 Tauri 窗口中的点击/悬停手感、中文输入法在覆盖层输入框中的行为、Tab 跨页滚动定位，以及系统菜单加速器的手动按压。
