# 12: 文本标记注释与注释列表

**What to build:** 用户可以选择文字并添加高亮、下划线、删除线；Annotation 记录作者与时间，支持颜色/透明度调整、列表定位与删除，并保存为 Native Annotation。

**Blocked by:** 07 (文本提取与文本层), 10 (Save As / Save Changes 管线)

**Status:** done

- [x] 选中文字后可添加高亮/下划线/删除线，颜色与透明度可调。
- [x] Annotation 自动记录作者（默认系统用户名，可改）与时间。
- [x] 注释列表可按 Page 分组、点击定位、选中与删除。
- [ ] 保存后 Annotation 是 Native Annotation，在 Preview、Adobe Acrobat Reader、Chrome 中可见且可编辑。
- [x] 撤销/重做覆盖注释的添加、修改与删除。

**Verification notes:**

- 合并提交 `6be5b03`（`git merge --no-ff ticket/12-text-markup-annotations`；分支 `a1d7e46`，feature `11471b5`，分支自身曾合并 integration `e3f837d` 并解决 `lib.rs` 命令注册冲突）。本次合并基于 `a4a0d58`（ticket 29 合并之后），与 ticket 29 仅 `.scratch`、`docs/spikes`、`spikes/` 文件重叠，实际无冲突。合并后未修改 spec、ADR、打包配置或其他 ticket。
- 勾选 1：`PageTextLayer` 在选区工具条中提供颜色（`#rrggbb`）、透明度（0–100%）与高亮/下划线/删除线按钮；`create_text_markup_annotation` 校验颜色、透明度、非空且有限正几何的矩形，并按选区逐行生成 `/QuadPoints`（每行一个四边形）。前端 `TextMarkup.test.tsx` 断言颜色/透明度随请求发送、三种类型分别创建、失败时保留选区并显示后端错误；Rust `underline_and_strikeout_are_native_annotations_with_visible_appearance_ink` 与 `annotation_opacity_is_visible_in_rendered_pixels_after_reopen` 断言保存重开后 PDFium 渲染出对应颜色笔画与 50% 黑色高亮的中间灰像素。
- 勾选 2：作者在创建时从持久化 `settings.annotationAuthor` 读取（`create_text_markup_annotation` 每次 `settings_store().load()`，前端不传入作者）；默认值改为系统用户名（`USER`/`USERNAME`，trim、非空、≤64 字符、无控制字符，否则回退 `PDFX 用户`），并保留设置页可改。`/CreationDate` 以 `D:YYYYMMDDHHmmSSZ`（UTC）写入。测试 `annotations_take_their_author_from_settings_at_creation_time` 断言 Alice→Bob 两次创建各自使用当时的设置、改设置不改写旧注释、默认作者等于 `default_annotation_author()`；`a_selection_highlight_saves_as_a_native_annotation_with_an_appearance_and_author` 断言保存后的原生字典含 `/T`、`/CreationDate`、`/NM` 并可重开读取。**ticket 16 勾选 3 由此可满足**（未修改 ticket 16）。
- 勾选 3：`list_annotations` 按文档页序返回所有有非空 `/NM` 的文本标记注释，`AnnotationPanel` 按 `pageIndex` 分组、显示类型/作者/时间；点击条目调用 `viewer.goToPage` 并在页面上绘制选中轮廓；选中后提供颜色/透明度（应用样式）与删除。`AnnotationPanel.test.tsx` 断言分组顺序、缺失作者/时间文案、样式调用与删除；`App.test.tsx` 的 `jumps to a listed annotation's page, outlines it and deletes it` 走完整 UI。无 `/NM` 的既有注释仍由 PDFium 渲染但不进入列表（无稳定身份与作者/时间元数据）。
- 勾选 4 **未验证（人工）**：保存路径已由自动化证明写出 Native Annotation：`a_selection_highlight_...` 用 lopdf 解析输出，断言 `Subtype`、`/NM`、`/T`、`/CreationDate`、`/QuadPoints`（每行 8 个数）与真实 `/AP` 正常外观流；`style_changes_...` 断言 `/C` 为 0/1/0、`/CA` 为 0.7、`/AP` 为流；ink/underline/strikeout 均有像素断言。但 Preview、Adobe Acrobat Reader、Chrome 中的可见性与可编辑性未在本机手动打开验证，故保持未勾选。PDFium 不会为文本标记合成外观，`/AP` 由适配层生成的页面坐标流（高亮填充四边形、下划线/删除线描边）经 vendored `FPDFAnnot_SetAP_str` 安装，透视 `/GS` ExtGState 实现 `/CA` 透明度。
- 勾选 5：新增注释、样式修改与删除都经 `push_annotation_action` 进入统一 `text_edit` 撤销栈（与文本框同一 `EditAction` 枚举、同一 dirty 基线）；样式修改通过“删除同名注释 + 按快照重建”实现，保留 `/NM`、作者与创建时间。`style_changes_are_undoable_and_saved_as_native_colour_and_opacity` 覆盖创建→改样式→撤销→重做→再撤销/重做；`deleting_an_annotation_is_undoable_and_restores_its_author_and_creation_time` 覆盖删除撤销/重做并恢复作者、创建时间、颜色与矩形；`saving_moves_the_dirty_baseline_without_clearing_annotation_history` 断言保存走 ticket 10 的 `save_document_with_progress` 后 dirty 归零、历史保留，撤销越过保存点重新变脏，重做回保存点恢复干净。权限检查按栈顶动作类型分别要求内容/注释权限。
- Vendored patch 审计：本 ticket 仅改 5 个 vendored 文件（新增 3 个 annotation 类型的 `close()`、`PdfPageAnnotation` 的 `/NM`、raw `/CreationDate`、任意字符串键读写），未改动其他 vendored 代码；`PDFX-PATCH.md` 同步更新为“八文件”并准确描述新补丁。补丁均为增量方法，无上游行为变化；Pin（0.9.4）与 `.crate` sha256 `8948a803...` 未变，`Cargo.lock` 仍为 path 依赖。`close()` 与既有 circle/ink 模式一致，创建句柄在 `set_bounds` 后立即关闭。
- 只读/权限：`ensure_annotation_editable` 使用 PDFium 注释权限（与内容编辑权限分离）；受限文档创建/改样式/删除返回 `permission_denied`，但列表仍可读。`read_only_documents_refuse_annotation_changes_but_still_list_them` 覆盖；前端无注释权限时隐藏控件并显示“此文档不允许添加注释”，面板显示“此文档不允许添加或修改注释”。扫描页（`hasText: false`）沿用既有早退分支显示“此页没有可编辑文字”，不显示选区与标记控件，因此无法选中文字添加注释。
- 依赖/许可/隐私：`src-tauri/Cargo.toml`、`Cargo.lock`、`package.json`、`package-lock.json` 相对 `e3f837d` 无变化，无新依赖（仅继续使用既有 serde/tauri/lopdf/png/base64）；无新许可证或 advisory。新增代码无网络、遥测或上传调用；默认作者只从本地环境读取并写入用户创建的注释，不写日志。界面文案为中文。
- 自动化验证（合并后在仓库根执行）：`npm run typecheck` 通过；`npm test` 22 文件 229 项全部通过（新增 `TextMarkup` 7、`AnnotationPanel` 6、`App` 1）；`npm run build` 通过；`npm run test:e2e` 12 项通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 4 条既有 dead_code warning）；`cargo test` 161 项通过、3 项 ignored（既有手动/延迟测试），其中 `text_markup` 9 项、`settings` 13 项。未触碰测试 fixture 与 manifest，无额外 fixture 检查。
- 已知限制/后续：
  - ticket 13：`TextMarkupKind`、`AnnotationSnapshot`、`capture_snapshot`、`list_annotations`、`with_text_markup!` 与 `AnnotationPanel` 目前都只覆盖高亮/下划线/删除线；便签与手绘需要扩展注释模型与列表类型，手绘的撤销快照需保存笔画而非四边形；ink 的创建/笔画/关闭已在 vendored patch 中存在，可复用适配层自由函数模式。
  - ticket 20：`annotations.rs` 以借用 `PdfDocument` 的自由函数为生产路径（打开文档注册表不能交出所有权），圆形创建目前只有 `PdfiumAdapter` 方法，需要补一个自由函数；形状注释也需扩展上述类型模型；边框须在 `/AP` 之前设置（Pdfium 设置边框会丢弃既有外观流），该约束已写入 `PDFX-PATCH.md`。
  - ticket 25：图章是原生注释，但作者/颜色/边界目前被 `with_text_markup!` 限制在三种文本标记内，需要扩展宏或走通用 trait；图章的外观（含透明度）可复用 `set_annotation_appearance`；列表与属性面板同样需要泛化。
  - 未验证的人工 GUI 项：真实 Tauri 窗口中的文字选择→标记交互、注释面板滚动与样式控件、以及在 Preview/Acrobat/Chrome 的跨阅读器矩阵（勾选 4）；自动化仅通过 mock IPC 与 PDFium 重开/像素断言覆盖。
