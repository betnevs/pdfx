# 31: 创建文本字段

**What to build:** 用户可以创建文本 Form Field，设置名称、提示、必填、多行、字号，保存后其他阅读器能识别并填写。

**Blocked by:** 29 (AcroForm 创建技术验证), 30 (填写已有表单 + 必填校验)

**Status:** done

- [x] 表单工具可放置文本字段并打开属性面板。
- [x] 名称、提示、必填、多行、字号可设置；名称重复时有提示。
- [ ] Widget 外观流正确，字段在 Preview、Adobe Acrobat Reader、Chrome 中可填写。（`/FT /Tx`、`/DA`/`/DR`、显式 `/AP /N`、CJK 子集与 90/180/270 旋转补偿已由 PDFium 像素与独立 lopdf 解析验证；Preview、Adobe Acrobat Reader、Chrome 未人工验证，保持未勾选）
- [x] 保存后字段值、外观与必填状态保留。
- [x] 撤销/重做覆盖字段创建、属性修改与删除。

## Verification notes

**Merge:** 先在 ticket worktree 把 integration `4ed3f10`（ticket 25 关闭后）合入分支 `39a3191`（feature `a060690`，此前已合入 integration `24e23ff`），得到 `0da66dc`（`merge: integration/pdfx into ticket 31 create text fields`）；再在 integration 执行 `eb5a31a`（`merge: ticket 31 create text form fields`，14 文件，+3959/−40）。合并后未修改 spec、ADR、依赖或打包配置；相对 `4ed3f10` 无 `Cargo.toml`/`Cargo.lock`/`package.json`/`package-lock.json` 变化。

**冲突与解决：** 两棵树均无文本冲突，git 自动合并；逐文件复核了语义冲突面：
- `src-tauri/src/text_edit.rs`：`EditAction` 并集为 `Box`/`Image`/`Field`/`FormStructure`/`Annotation(Box<AnnotationEditAction>)`。ticket 25 只把 `Annotation` 装箱，未改联合；本票新增 `FormStructure(FormStructureEditAction)`，`undo_for`/`redo_for` 把 Box/Image/FormStructure 归为内容修改权限、Field 归为填写权限、Annotation 归为注释权限，保留“先看栈顶再弹出”的既有分派。`push_form_structure_action` 与 Field/Box/Image/Annotation 共用同一 `EditSession`、`undo_stack`/`redo_stack`、`current_state`/`saved_state`，没有第二撤销栈。
- `src-tauri/src/lib.rs`：同时保留 `form`/`form_create`/`image_edit`/`stamps`/`annotations` 模块注册与 ticket 25 的全部命令；新增 `create_form_text_field`/`update_form_text_field`/`delete_form_text_field` 三个命令。
- `src-tauri/src/save.rs`：`ObjectEdit` 新增 `CreateTextFields { fields }`，`save_document_with_progress` 依次合并 `entry.save.object_edits`、`form::save_object_edits`、`form_create::save_object_edits`，仍只调用一次 `append_object_edits`（一个 lopdf revision）且派生自 PDFium 的增量输出。`ObjectEdit` 的 `Eq` 因 `TextFieldSpec` 含 `f32` 改为 `PartialEq`，无调用点依赖 `Eq`。
- `src-tauri/src/form.rs`：保留 ticket 30 的发现/填写/必填门禁，新增 `discover_all`（PDFium widget + 会话创建字段合并）、`FormFieldView` 的 `tooltip`/`font_size`/`created` 字段；`form_fields` 与 `missing_required_fields` 都改走 `discover_all`，`fill_for` 对 `created` 字段走会话快照写入。ticket 25 未触达该文件，自动合并正确。
- 前端：`PageViewer.tsx` 同页渲染 `PageImageLayer` → `FormLayer`（新增 `canModifyContent`）→ `ShapeLayer` → `StampLayer`（ticket 25），各层保留自己的权限与选择状态；`App.tsx` 同时向 `PageViewer` 传 `canFillForms`/`canModifyContent` 与 `armedStamp`；`backend.ts` 的表单/图章/注释类型并集无重复定义；`styles.css` 同时保留表单、图章与注释样式；`FormLayer.test.tsx` 的 `FormFieldView` fixture 补齐 `tooltip`/`fontSize`/`created` 后与既有测试兼容。
- Vendored patch：ticket 31 feature 未改 `src-tauri/vendor`；合并后 `PDFX-PATCH.md` 仍为 **thirteen files** 与 **eight close types**（ticket 25 的 `stamp.rs` close 为第 13 个文件/第 8 种类型），与 ticket 25 关闭记录一致。本票未新增 vendored hunk。
- 回归：合并后 ticket 25 的图章、ticket 13 的便签/手绘、ticket 20 的形状、ticket 30 的表单填写、ticket 14 图片、ticket 11 字体、ticket 10 保存与 ticket 12 文本标记测试全部通过；无第二历史、无重复命令注册。

**放置与属性面板：** `FormLayer` 的“添加文本字段”按钮进入放置模式，页面上单击使用默认 180×24 pt、拖动使用拖出矩形（最小 12 pt 触发拖拽、后端最小 4×4 pt 且必须位于页面内），成功后立即选中新字段并打开属性面板。`FormLayer.test.tsx` 的 “places a text field by dragging and opens its property panel” 断言放置请求的显示矩形、生成名 `text_field_1`、属性面板初值（名称/提示/必填/多行/字号 12）以及退出放置模式；Rust `creates_a_text_field_that_discovery_reports_and_undo_removes` 从命令边界断言 `new:` 字段立即出现在 `form_fields` 且 undo/redo 往返。属性面板可改名称、提示文字、必填、多行与字号（前端 4–200 校验并显示“字号必须在 4 到 200 点之间。”，后端 `normalize_font_size` 同样限制 4–200，默认 12）。`updates_and_deletes_created_fields_through_the_history` 与 `FormLayer.test.tsx` 的 “updates created field properties from the panel” 覆盖各属性更新、重命名、多行控件切换与历史记录。

**名称校验/重复名：** 后端 `validate_name` 拒绝空/纯空白、控制字符与超过 128 字符的名称（`form_field_name_invalid`，中文消息）；`create_for` 用 `discover_all` 的名称集合同时覆盖已有 PDFium widget 与会话字段，`update_for` 排除自身后覆盖两者，命中返回 `form_field_name_duplicate`（“表单字段名称“X”已存在，请换一个名称。”），且在改动会话存储与历史之前返回。测试 `rejects_invalid_bounds_and_duplicate_names_without_creating_a_field` 覆盖会话内重复、空白名与越界矩形且断言历史 revision 不变；`updates_and_deletes...` 覆盖重命名到会话内另一字段被拒且原名不变。审计临时探针（已删除，不提交）在 `form-acroform.pdf` 上验证：以已有 widget 名 `full_name` 创建与把新字段改名为已有 `email` 都返回 `form_field_name_duplicate`、消息含“已存在”、字段数不变、历史 revision 仍为 0；`FormLayer.test.tsx` 的 “keeps the field unchanged when the backend rejects a duplicate name” 断言前端保留草稿、显示中文错误且不产生第二个字段。

**发现/顺序/填写/必填门禁：** `merge_views` 按页序合并，同一页先已有 widget、后会话创建字段，再统一重编连续 `order`；`FormLayer` 的 Tab 顺序直接使用该数组（跳过只读），跨页仍经 `onSelectPage`/`onFocusField`。`fills_a_created_field_and_the_required_gate_follows_the_value` 断言新字段在保存前即可填写、`missing_required_fields` 从 1 变 0、undo 恢复空值后门禁重新出现、redo 再清空；`fill_for` 对 `created` 字段只接受文本并在会话中保存值，保存时写入 `/V` 与外观。必填 Save/Save As 门禁仍是 ticket 30 的 `useDocumentSave.checkRequired`（保存前调用 `missing_required_fields`），因该命令已改用 `discover_all`，新字段无需额外接线即被覆盖；`FormLayer.test.tsx` 确认新字段的 `<input>` 立即启用，可与既有 widget 一同填写。

**保存结构与跨阅读器证据：** `saves_a_created_field_with_ap_da_dr_and_a_cjk_appearance` 在 Save As 后用 PDFium 重开并独立 lopdf 解析：`/FT /Tx`、`/Subtype /Widget`、唯一 `/T`、UTF-16BE `/TU`（“请输入姓名”）、`/Ff` Required bit（单行时 Multiline bit 为 0）、`/DA` 含 `PdfxNoto` 与字号、`/Q 0`、`/F 4`、`/Border`、`/Rect`、`/P` 指向页面、显式 `/AP /N`；`/AP` Form XObject 的 `/BBox` 与 `/Rect` 相同、内容含 `BT /PdfxNoto 12 Tf` 与 `Tj`、未旋转矩阵 `1 0 0 1 72 668 cm`；`/DR /Font /PdfxNoto` 是 Type0/Identity-H + CIDFontType2（`CIDToGIDMap`、`W`、`FontDescriptor /FontFile2`、`ToUnicode` 含 `<5F20>`/`<4E09>`）。审计探针确认新建字段是无 `/Parent` 的 terminal field，`/AP` 的 Resources/Font 引用同一子集字体；页面 `/Annots` 与 `/Fields` 各增加 1，PDFium 重开后字段可发现、可填、required 状态与值保留，且 `created == false`。`saves_an_empty_created_field_with_a_static_appearance` 断言空值不写 `/V` 但仍有 `/AP /N` 与可用字体子集。`rotated_pages_get_upright_compensated_appearances` 在 `/Rotate` 90/180/270 + CropBox 的合成页上断言 `/Rect`/`/BBox` 与补偿矩阵（`0 1 -1 0 …`、`-1 0 0 -1 …`、`0 -1 1 0 …`），并用 PDFium 渲染像素断言文字横向直立而非侧躺。`/NeedAppearances` 在新建表单上从不设置；合并已有 AcroForm 时保留源值（探针比较源/输出）。勾选 3 仍保持未勾选，见文末人工矩阵。

**保存管线/重复保存/保真：** 本票没有独立保存路径，只经 ticket 10 的 `save_document_with_progress`；创建字段作为一个 `ObjectEdit::CreateTextFields` 与既有表单对象编辑一起进入同一次 `append_object_edits`。审计探针在 `form-acroform.pdf` 上 Save As 后统计 `startxref`：源 +2（PDFium 一次增量 + lopdf 至多一次 revision），输出以源字节为前缀，源文件字节与 mtime 均未变，输出中不存在任何 `new:` 会话 id。`repeated_saves_never_duplicate_created_fields` 连续 Save As/Save Changes 并重开：`/Fields` 与页面 `/Annots` 各保持每个创建字段一次，撤销创建后保存会移除该字段与 `/Annots` 项、重做后恢复一次；保存不清空创建字段的会话记录，因此保存后仍可继续属性修改/删除并反映到下一次保存。`merges_created_fields_into_an_existing_acroform` 断言合并后已有字段（`full_name` 值 “Ada Lovelace”、`email`、`subscribe`、`gender`、`country`、`department`、`notes`）与 `/DR` 既有字体保留，`/Fields` 等于源条目数 +1；探针进一步确认源 `/DA` 与源 `/NeedAppearances` 原样保留。代码只写 AcroForm/Fields/DR/DA、widget 与字体对象，从不写 `/AA`、`/K` 或任何 JavaScript 动作。

**撤销/重做与权限：** 创建、属性修改、删除、填写统一为 `EditAction::FormStructure`，动作携带整条 `CreatedTextField` 快照；`undo_form_structure`/`redo_form_structure` 直接恢复会话记录并同步 `current_state`，因此字段列表、几何、属性、值与外观一起回退。`updates_and_deletes_created_fields_through_the_history` 覆盖属性更新、重命名、删除与 undo/redo；`fills_a_created_field...` 覆盖值填写。权限分派：创建/属性修改/删除/对创建字段的填写在命令入口要求内容修改权限（`ensure_content_editable`，中文“此文档的权限限制不允许创建表单字段。”），undo/redo 的 FormStructure 分支同样要求内容权限；已有字段的填写与填写撤销仍走 `EditAction::Field` 的表单填写权限。由于创建字段只能在允许内容修改的文档中产生，其填写的撤销在同类文档中始终可用；现有 fixture 没有“允许填写但禁止修改内容”的文档，未构造该组合。`permission_restricted_documents_refuse_creation_with_a_chinese_reason` 断言 `encrypted-owner-restrictions` 上创建返回 `permission_denied`、中文原因、字段列表为空、历史 revision 0 且不 dirty；前端在 `canModifyContent == false` 时禁用“添加文本字段”并显示“此文档的权限限制不允许创建表单字段”（`FormLayer.test.tsx` 覆盖）。`scanned_pages_accept_created_fields` 在纯扫描页（无文本层）上创建、填写、保存并重开成功。

**加密源 + 创建字段 + Save As：** 已知缺口，按任务要求记为人工/未测，不作为阻塞项。加密源保存仍需 `confirm_unencrypted_output`（ticket 10/ADR-0007），`serialize_output` 走 `FPDF_REMOVE_SECURITY` 全量重写而非增量；现有加密 fixture 中 `encrypted-owner-restrictions` 禁止内容修改（创建在入口即被拒），`encrypted-print-denied` 允许修改但未跑“创建字段 + Save As 确认”端到端。建议在后续表单/保存票补一张“加密且允许修改内容 + 已有表单”的 fixture 并纳入人工矩阵。

**中文/隐私/依赖/seam/范围：** 新增 UI 文案与用户可见错误均为中文；后端新增错误码 `form_field_name_invalid`/`form_field_name_duplicate`，前端经 `toCommandError` 直接展示后端中文消息。`form_create` 只使用本地 PDFium/lopdf/既有 `fonts::noto_sans_sc()` 子集，无网络、遥测、上传或字段值日志。相对 `4ed3f10` 无新增 Rust/npm 依赖（`ttf-parser`、`lopdf` 均为既有依赖）。新增能力仅经 `create_form_text_field`/`update_form_text_field`/`delete_form_text_field` 三个 Tauri 命令暴露，前端只经 `src/lib/backend.ts` 调用，跨工具刷新只经 `documentEvents` 的 `source: "form"`。未实现 ticket 32/33 的复选框、单选、列表/下拉、签名字段创建（`form_create` 只处理文本；`CreateFormTextFieldRequest` 无字段类型参数）。

**观察（非阻塞）：** 表单放置模式与图章 armed 状态各自独立；若用户先选图章再点“添加文本字段”，两层同时接收时位于上层的 `StampLayer`（z-index 3）会截获页面点击。这与既有形状/图章等工具间缺少全局“当前工具”互斥是同一类问题，不是本票引入；未在本次合并中修改，建议后续统一工具状态时处理。

**自动化验证（合并后在 integration 树执行）：** `npm run typecheck` 通过；`npm test` **32 文件 / 323 项**全部通过（较 ticket 25 关闭时 318 项净增 5 项，全部为 `FormLayer.test.tsx` 创建字段用例）；`npm run build` 通过；`npm run test:e2e` **12 项**通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 既有 4 条 warning）；`cargo test` **234 通过 / 0 失败 / 3 ignored**（较 ticket 25 关闭时 223 通过净增 11 项：`form_creation` 11 项；`form_fields` 10、`save_document` 22、`text_edit` 18、`stamps` 9、`note_ink` 16、`shapes` 8、`image_insertion` 13 全部通过，无回归）。worktree 侧同一门禁亦通过。`fixtures/generate.py` 与 fixture 未修改，未运行 fixture 再生成。

**人工未验项：** Preview、Adobe Acrobat Reader、Chrome 中创建字段的显示与填写（勾选 3 保持未勾选）；真实 Tauri 窗口中的拖放手感、中文输入法在新建字段覆盖层输入框中的行为、属性面板焦点与 Tab 跨页；加密源 + 创建字段 + Save As 确认路径。自动化仅覆盖 PDFium 渲染、PDFium 重开与独立 lopdf 结构。

**带入 ticket 32/33：** 直接复用 `CreatedFieldStore`/`CreatedFieldSnapshot`/`EditAction::FormStructure`/`merge_views` 与 `FormStructureEditRequest` 面板模式；复选框/单选需写 `/FT /Btn`、`/Ff` radio/combo bits、`/AP /N` 的 `/Yes`/`/Off` 状态与 radio 的 `/Kids`/`/AS`，并保持父 `/V` 与每个 kid 的 `/AS` 一致；列表/下拉需写 `/Opt`、`/V` 与 `/I`，其值外观也要进同一字体子集管线；签名只建带边框的空结构（可沿用本票显式 `/AP` 或按 ticket 29 建议补 `/MK`）；所有新类型必须继续出现在 `discover_all` 的同一列表与同一 `order` 中，才能被必填门禁与 Tab 顺序看到；删除/创建仍使用内容权限，填写已有字段仍使用表单填写权限；保存继续在同一个 `ObjectEdit` 家族与同一 lopdf revision 内扩展，确保重复保存不重复写对象；名称唯一性检查需同时覆盖已有 widget 与会话字段。人工矩阵（Preview/Acrobat/Chrome、加密源组合）应随 32/33 一并补齐。
