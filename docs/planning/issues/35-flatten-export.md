# 35: Flatten 注释导出

**What to build:** 用户可以把视觉 Annotation（高亮、下划线、删除线、便签、手绘、形状、图章、高级注释）Flatten 成页面内容的副本；链接与 Form Field 保持可交互。

**Blocked by:** 12 (文本标记注释与注释列表), 13 (便签与手绘注释), 20 (矩形与椭圆形状), 25 (图章), 34 (高级注释)

**Status:** done

- [x] 导出副本中视觉 Annotation 变成页面内容，任何阅读器显示一致。
- [x] 链接与 Form Field 不被 Flatten，仍可点击/填写。
- [x] 原 Document 不被修改；Flatten 只写新副本。
- [x] 便签等无页面外观的注释有明确处理（渲染图标或省略）。
- [ ] 导出结果在 Preview、Adobe Acrobat Reader、Chrome 中验证。（人工项，未执行；见“人工未验项”）

**Verification (agent):** 分支 `ticket/35-flatten-export`，功能提交 `1a35297`；先合入 integration tip `b8a1be2`（仅 `.scratch/pdfx/spec.md` 的文档改动，合并提交 `29c4ee9`，无冲突）。主仓库 `integration/pdfx` 以 `git merge --no-ff` 合入为合并提交 `347d1cb`（合入时 tip 仍为 `b8a1be2`，票 45 未落地，无冲突）。合并后独立审计发现并修复一处真实缺陷，修复提交 `b8a9ff7`；关闭提交为 `chore: close ticket 35`（即本文件的最后一次修改）。

相对 integration tip `b8a1be2` 的代码改动为 12 个文件（+3675/−212，含审计修复的 +787/−6）：`flatten_export.rs`、`atomic_write.rs`、`save.rs`、`print_export.rs`、`commands.rs`、`lib.rs`、`tests/flatten_export.rs`、`PrintExport.tsx`、`PrintExport.test.tsx`、`backend.ts`、`backend.test.ts`、`styles.css`；另更新了本票文件。未改 `src-tauri/vendor`（`PDFX-PATCH.md` 仍为 15 文件 / 10 个 close 类型，vendor diff 为空，无新 hunk）、`Cargo.toml`/`Cargo.lock`/`package.json`/`package-lock.json`、spec、ADR、`tauri.conf` 或 capabilities，未新增依赖，无网络/遥测。

**实现：** 新命令 `export_flattened_copy(document_id, destination, confirm_unencrypted_output)` 通过统一命令边界测试。命令先拒绝空目标与 Source/当前 Output（复用 `target_path::same_file`，覆盖符号链接与大小写变体），再用新提取的 `save::serialize_live_document` 把当前 live 状态（含未保存编辑、表单与对象编辑）序列化为内存字节，全程不触碰 PDFium 文档、历史或 Source；随后 `flatten_document` 用 lopdf 解析，仅对含视觉注释的页克隆页面并追加一个 lopdf 增量 revision：把每个注释可用的 `/AP /N` Form XObject 复制为独立对象，加入页面（含 `/Parent` 继承）`/Resources /XObject`（唯一 `PDFXFlatN` 名），按「（BBox×/Matrix）包围盒 → `/Rect`」写 `cm`（外观自身 `/Matrix` 不重复写入，只由 `Do` 应用一次）与 `q … Do Q` 到单个内容流，并从该副本的 `/Annots` 移除已绘制注释；最后用共享的 `atomic_write::write_atomically` 写同目录临时文件、`sync_all`、`rename`，失败不留半成品（`print_export` 也改用该 helper，删掉重复实现）。原页面 `/Contents` 支持缺失、单流、数组、直接对象与 Null，未改动页保持原内容流字节一致；页面 `/Rotate` 无需额外变换（注释与页面内容共用未旋转用户空间，旋转由阅读器统一应用），已由 90/180/270 渲染用例验证。分类策略：`/Link`、`/Widget`（含签名字段 widget）原样保留并计入 `preserved`；`/Popup`、隐藏/不可见/NoView 注释、无可放置外观的注释（无 Form AP、图像 AP、BBox 无效）被省略并逐条返回 `skipped`（`popup`/`hidden`/`no_appearance`/`unsupported_appearance`），不会留下可能损坏副本的残留；PDFX 便签自带图标 AP，因此按普通视觉注释 flatten；若注释带有关联 `/Popup`，该 Popup 因无页面外观而省略。加密 Source 沿用 ADR-0007：未显式确认时返回既有 `unencrypted_output_confirmation_required` 且不写文件，确认后只写未加密副本、源文件字节与 mtime 不变。

**独立审计（合并后）：** 在 `347d1cb` 上独立复核并新增 5 项 Rust 用例（`tests/flatten_export.rs` 共 15 项），据此定位一处真实缺陷：原实现把外观流自身的 `/Matrix` 也乘进写入页面的 `cm`，而 `Do` 按规范已应用该 Matrix，导致 `/Matrix` 非恒等的外观被二次变换、位置错误。PDFX 原生注释外观均为恒等 Matrix，故功能分支自带用例未暴露；审计用例 `foreign_appearance_bbox_and_matrix_placement_matches_pdfium_rendering` 用普通/缩放/旋转/自带字体资源的外来外观复现（修复前旋转方块 MAE 2.94 且像素包围盒偏移，修复后 MAE 0 且与 `/Rect` 对应），修复提交 `b8a9ff7`。其余审计结论与新增用例如下。

**逐项行为（独立验证）：**

1. **视觉注释 → 页面内容：** 仅含视觉注释的页被克隆并追加一个 lopdf 增量 revision；每个可用 `/AP /N` Form XObject 以唯一 `PDFXFlatN` 名加入（含继承）`/Resources /XObject`，`cm` 把（BBox×Matrix）包围盒映射到 `/Rect`，`q … Q` 包裹的 `/PDFXFlatN Do` 绘制，外观自身资源（如 `/Font`）可解析。审计覆盖：四色外来外观 flatten 前后 PDFium 渲染 MAE < 2 且每色像素包围盒与理论 `/Rect` 一致（±3px）；`/Rotate` 0/90/180/270 + CropBox [50 80 450 680] 渲染等价；文字外观仍以文字层存在（不栅格化）；`/Contents` 缺失/单流/数组/直接对象/Null 与继承资源均正确接续，未改动页内容流字节不变；既有图形状态由 `q/Q` 隔离，未引入 `/NeedAppearances`；书签、元数据与既有链接保留。
2. **链接与 Form Field：** `/Link` 与 `/Widget`（含签名 widget）保留在 `/Annots` 与 `/AcroForm`，保存并重开后 12 个字段（文本、复选框、单选、下拉/列表）仍被识别，文本字段可继续填写；签名 widget 保持 `/FT /Sig`、无 `/V`。审计用例 `visual_annotations_flatten_on_a_page_that_also_keeps_form_widgets`：同页 12 widget + 1 链接 + 1 视觉注释时只 flatten 视觉注释（flattened=1，preserved=13），重开 `form_fields` 仍为 12。
3. **原 Document 不被修改：** `serialize_live_document` 为 `save_document_with_progress` 的原样提取，审计用例 `flatten_export_leaves_save_output_and_save_state_unchanged` 执行「save_as → flatten 导出 → save_as」，两次保存字节除 PDFium trailer `/ID` 外完全一致、编辑历史不变，且保存后的 PDF 仍含原始 Highlight 注释、无 `PDFXFlat`（flatten 不污染保存路径；Source/current Output/live 文档与撤销历史另有既有用例覆盖）。目标保护复用 `target_path::same_file` 拒绝 Source/当前 Output（含符号链接与大小写变体；硬链接别名见限制）；写盘经同目录临时文件 + `sync_all` + `rename`，失败不留半成品（`print_export` 改用共享 `atomic_write`，其 11 项既有用例仍通过）。
4. **便签与省略策略：** PDFX 便签自带图标 AP，按普通视觉注释 flatten（图标绘入页面，报告 flattened）；审计用例 `flattening_a_note_draws_its_icon_and_leaves_no_orphan_popup` 确认 `/Popup` 以 `popup` 原因跳过、副本无 `/Annots`（无孤儿 Popup）。无可用 Form AP、图像 AP、无效 BBox、隐藏/NoView 注释逐条以 `skipped` 及原因返回，不静默丢失、不写入副本。加密 Source 未确认返回 `unencrypted_output_confirmation_required` 且不写文件，确认后只写未加密副本、源字节/mtime 不变。

**门禁（合并 + 审计修复后，主仓库 `b8a9ff7`）：** `npm run typecheck` 通过；`npm test` **37 文件 / 427 项**通过（与分支报告一致）；`npm run build` 通过；`npm run test:e2e` **12 项**通过；`cargo fmt --check` exit 0；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 既有 4 条 warning，未用 `--all-targets`）；`cargo test` **303 通过 / 0 失败 / 3 ignored**（较 integration tip 的 288 净增 15：票内 10 + 审计 5），其中 `tests/flatten_export.rs` **15 项**全部通过。审计新增用例：`foreign_appearance_bbox_and_matrix_placement_matches_pdfium_rendering`、`content_stream_shapes_and_inherited_resources_survive_flattening`、`flatten_export_leaves_save_output_and_save_state_unchanged`、`visual_annotations_flatten_on_a_page_that_also_keeps_form_widgets`、`flattening_a_note_draws_its_icon_and_leaves_no_orphan_popup`。

**人工未验项（勾选项 5 未勾选）：** 导出副本在 Preview、Adobe Acrobat Reader、Chrome 中的真实阅读器矩阵未人工验证；真实 Tauri 窗口中的保存对话框与中文提示手感亦未人工核验。自动化仅覆盖 PDFium 渲染/重开与像素对比、独立 lopdf 结构、命令边界与 jsdom 前端交互。

**已知限制/后续：** 不栅格化，完全以注释自身 `/AP /N` 为准；无可用 Form AP 的外来视觉注释按报告策略省略而非绘制占位；隐藏注释与 Popup 不写入副本；`same_file` 只检测同一路径、符号链接与大小写变体，硬链接别名不会被拒绝；签名字段仅保留结构，不执行证书签名；加密源确认后副本不加密。
