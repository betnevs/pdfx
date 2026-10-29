# 22: 选区级混合文本样式

**What to build:** 用户可以在一个 Text Box 内选中部分文字，应用不同的字体、字号、颜色、粗体/斜体/下划线，形成混合样式并保存。

**Blocked by:** 09 (新增 Text Box 与样式), 11 (CJK 字体回退与子集嵌入)

**Status:** done

- [x] 选区级样式修改只影响选中文字，不影响同框其他文字。
- [ ] 混合样式在画布、保存后 PDFium 提取与外部阅读器中一致。（画布像素、PDFium 图层/提取与外部 poppler `pdftotext` 已一致；Preview、Adobe Acrobat Reader、Chrome 的图形界面未人工打开验证，保持未勾选）
- [x] 字体切换时缺字形回退提示与整框模式一致。
- [x] 撤销/重做按样式操作粒度工作。
- [x] 不引入段落 reflow；行高与换行保持 Text Box 规则。

**Verification notes:**

- 合并与分支：ticket 分支 `ticket/22-selection-text-styling`，最终提交 `a9616ac`（此前 `70ca86d`、`2c545b3`），以 `--no-ff` 合入 `integration/pdfx`，合并提交 `de1080b`；此前的 `8d0b488`、`0cde0cb` 已分别合入前两个提交。未 push，未改动其他 ticket。
- 审计方式：把实现者报告当作待证声明，不依赖其摘要。所有结论都经 Tauri 命令边界驱动（创建 Text Box → 输入 → 选区样式 → 保存 → 重开），并用独立 lopdf 解析、PDFium 图层/像素断言和外部 poppler `pdftotext -enc UTF-8` 交叉验证。审计中发现并修复了两个真实缺陷：
  1. **PDFium 按 BaseFont 去重导致字体子集串用**：`CPDF_PageContentGenerator::ProcessText` 用 `CPDF_PageObjectHolder::FontsMapSearch` 按 `(BaseFont, type)` 复用字体，同一页上两个不同的 Noto 子集都声明 `/BaseFont NotoSansSC-Thin`，第二个子集会经第一个子集的 CID 映射解码（组合附加符单独成 run、同页两个 CJK Text Box 均可复现）。修复 `70ca86d`：为每个子集生成基于内容哈希（FNV-1a 64）的唯一 PostScript 名（`name` 表 ID 6），重写 sfnt 表目录、按 tag 排序、`head.checkSumAdjustment` 置零计算校验和、整字体校验和 0xB1B0_AFBA；`is_bundled_family` 压缩字母数字后比较，小写十六进制后缀不会误判 family 或 Bold/Italic。回归测试：`bundled_subsets_get_unique_stable_postscript_names`、`renamed_subsets_keep_a_valid_sfnt_checksum`、`two_noto_subsets_in_one_box_keep_distinct_pdf_fonts`、`a_combining_mark_only_run_keeps_its_own_subset_and_reopens_intact`、`two_noto_boxes_on_one_page_do_not_share_a_subset_font`。
  2. **混合样式分段忽略尾随空格 advance**：分段位置原先取 `PdfPageTextObject::bounds()`（墨迹边界，不含尾随空格和字形边距）。`"Selected line."` 把 `"line"` 设为 Times 24 后，run 起点落在前一词墨迹右缘（133.536），空格在 134.256 处没有布局效果；PDFium 提取仍含空格，但外部 poppler 提取为 `"Selectedline."`。修复 `a9616ac`：vendor 增加 `PdfFont::character_advance()`（`FPDFFont_GetGlyphWidth()` 的 glyph 参数按 Unicode 码点解析，已对照 PDFium `fpdf_edittext.cpp`），`measure_segment_advance()` 逐字符累加 advance（无法测量时回退墨迹宽度），分段与对齐行宽均改用 advance。`PDFX-PATCH.md` 的 vendored 文件数更新为 17。回归测试 `a_styled_run_starts_after_the_trailing_space_of_the_previous_run` 断言 run 起点等于空格 origin + width；重新生成同一 PDF 后 `pdftotext -enc UTF-8` 输出 `"Selected line."`，中文同样完整。
- 勾选 1（只影响选区）：`styling_a_character_range_keeps_the_rest_of_the_box_unchanged` 断言范围外字符仍为 16pt 黑色、范围内为 24pt 红色并渲染出红色像素；`overlapping_selections_split_straddling_runs_and_merge_equal_neighbours` 覆盖跨 run 的相邻/重叠选区拆分与等样式合并；`a_legacy_text_object_can_style_a_selection_without_moving_the_rest` 覆盖既有 Text Object；前端 `TextBoxEditing.test.tsx` 覆盖折叠光标/编辑器外整框样式、混合样式提示与忙碌禁用。折叠光标（selection 为 `null`）走整框路径，编辑器外同理。
- 勾选 3（回退提示一致）：`a_run_falls_back_to_the_bundled_font_with_the_same_notice_as_a_whole_box` 断言选区级回退与整框使用同一 `FontUsageView`（原 family、实际 family、reason），并把实际生效 family 持久化进 run，重开后 `text_edit_target` 仍报告 `"Noto Sans SC"`；Noto 无斜体时选区路径同样返回 `UnsupportedFont`。`TextBoxStylePanel.tsx` 合并了 `ef40f51` 的意图：不在可选列表中的 legacy family 从第一个可选 family 起步，而不是把原始名当作可提交值（`src/components/TextBoxStylePanel.tsx` 的 `selectedFamily` 回退）。
- 勾选 4（撤销/重做粒度）：`one_undo_restores_the_previous_runs_and_redo_reapplies_them` 逐次撤销/重做单次样式操作，断言 runs 精确恢复；`undo_and_redo_styled_ranges_reach_past_a_saved_revision` 覆盖保存后仍可撤销/重做；`transforming_a_styled_box_keeps_its_runs_and_undo_restores_them` 覆盖变换与 runs 快照。每次样式操作只 push 一个 `EditAction::Box`（text/layout/style/runs/created/deleted/record_index 前后快照）。
- 勾选 5（不 reflow）：`a_mixed_size_run_wraps_at_the_box_width_and_keeps_the_line_height_rule` 断言混合字号行按原 box 宽度换行、行高 = 该行最大字号 × 1.2、基线 = 行顶 + 最大字号 × 0.8；`mixed_styles_keep_the_box_alignment_on_the_line` 断言居中行以整行 advance 居中于 box 中心（容差 0.5）；没有按样式重排段落，换行宽度仍取 Text Box 的 `layout.width`。
- 边界与错误路径：前端 textarea 的 UTF-16 选区经 `[...text]` 转成码点（`TextBoxEditing.test.tsx` 的 `"A😀B\nC"` 用例断言 `3..6` → `2..5`，代理对只计一次）；后端 `TextRangeView` 文档明确为 Unicode 标量值，CRLF/裸 CR 在 `apply_text_edit` 中归一化为 `\n`；`a_combining_mark_only_run_keeps_its_own_subset_and_reopens_intact` 覆盖组合附加符单独成 run（不 panic、不拆错字符）；`selecting_the_whole_box_and_emptying_it_resets_runs_without_dangling_ranges` 覆盖全选→清空后 runs 为空、保存重开不残留字符；`retyping_the_same_text_keeps_runs_while_a_replacement_resets_them` 覆盖删除选区、覆盖输入、拆分/合并 run 与空文本；`invalid_ranges_and_styles_are_rejected_without_mutation` 断言越界/逆序 range 与非法样式返回 `InvalidArgument` 且无变更；`permission_restricted_documents_reject_selection_styling` 断言只读/无权限文档返回 `PermissionDenied`，不修改内容也不新增历史。legacy Text Object 保持 ticket 43 行为：可选 family 可整框/选区重设样式；未提供的嵌入 family 被拒绝且不变更（`legacy_embedded_fonts_refuse_selection_styling_without_mutating`）。
- 保存保真：`mixed_styles_survive_saving_and_reopening_with_one_lopdf_revision` 断言 Source Document 字节与 mtime 未变、输出只追加 PDFium 与 lopdf 各一层（`startxref` 计数 ≤ 源 + 2）、独立 lopdf 解析看到 Helvetica 与 Times 两个字体资源、重开 PDFium 后文字/字号/颜色/红色像素与下划线都在。审计另用临时探针（已删除、未提交）在同一会话连续保存三次：源文件字节不变，三次输出的 `startxref` 数相同、重开后图层文字与字符数逐次相同且空格保留，说明重复保存结构稳定、不会堆叠 revision 或重复 run。真实写盘 `save_document` 管线由 ticket 10 的测试覆盖，本票只新增内容对象，不新增保存路径。
- 门禁（在最终 integration tip `de1080b` 执行）：`npm run typecheck` 通过；`npm test` **51 文件 / 632 项**通过；`npm run build` 通过；`npm run test:e2e` **12 项**通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 4 条既有 dead_code warning，符合预期）；`cargo test` **509 通过 / 0 失败 / 3 ignored（41 个 suite）**，其中 `text_selection_styles.rs` 19 项全部通过。ticket worktree 合入前同一套门禁计数相同。
- 未人工验证：真实 Tauri 桌面窗口中的拖选/输入手势与编辑器交互（Playwright e2e 不覆盖文本选区）；Preview、Adobe Acrobat Reader、Chrome 中保存结果的图形界面观感（外部验证使用 poppler `pdftotext` 与 PDFium，未打开这三个阅读器）；CJK 扩展 B 未单独造字测试，其代理对换算与 emoji 用例走同一条前端码点转换路径。
