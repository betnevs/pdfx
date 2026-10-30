# 09: 新增 Text Box 与样式

**What to build:** 用户可以在 Page 上新增 Text Box，移动、缩放、旋转，并整框修改字体、字号、粗体/斜体/下划线、颜色与对齐；输入超长时自动向下扩展换行。

**Blocked by:** 08 (编辑已有文字 + 撤销/重做)

**Status:** done

- [x] 新增 Text Box 后可输入文字，默认样式可见；移动/缩放/旋转可逆。
- [x] 整框样式修改立即生效；字体列表只包含内置可自由嵌入字体。
- [x] 输入超过框宽时自动换行并向下扩展，不自动缩小字号；手动缩小时才裁剪。
- [x] 文本溢出、空框、删除与撤销/重做行为一致。
- [ ] 新增的 Text Box 在保存后于 Preview、Chrome 中显示位置与样式一致。

**Verification notes:**

- 合并提交 `1b38dcb`（`ticket/09-text-boxes` `e33f7c3`，基于 `ticket/08-edit-text-undo` `c3051ef`），`git merge --no-ff` 无冲突。ticket 08 已先按同一合并关闭（`01faa31`）。未修改 spec、ADR 或其他 ticket。
- 勾选 1：`creating_a_text_box_and_typing_survives_saving_and_reopening` 断言默认 Helvetica 16/黑色/左对齐可见、空框列入 overlay、输入后第一行基线为框顶 + 0.8em、保存重开后位置/字号/颜色一致。`move_resize_and_rotate_are_reversible_and_survive_saving` 断言移动/旋转可 undo/redo 且保存后矩阵与朝向保留；`resizeBox`/`moveBox`/`rotationFromPointer` 有独立单元测试。前端 `TextBoxEditing.test.tsx` 覆盖拖拽创建、选中、样式面板与删除。
- 勾选 2：`whole_box_style_changes_apply_immediately_and_survive_saving` 断言 Times/Bold/Italic/Underline/颜色/右对齐立即反映到像素与保存后的目标视图。字体注册表仅 12 个 PDF base-14 变体（Helvetica/Times/Courier），`source=base14`、`embedded=false`、`embeddable=true`、`coverage=win_ansi`（`font_registry_lists_only_base14_families_with_metadata`），不会引用许可不明确的系统字体。已知限制：ADR-0006 的 Noto/Inter/Liberation TrueType 子集嵌入尚未实现，属性面板显式提示“仅支持 PDF 基础字体”，由 ticket 11 扩展；`TextBoxFontView` 的 `source`/`embedded`/`coverage` 字段已为其预留。
- 勾选 3：`auto_height_grows_for_overflow_and_never_shrinks_automatically` 断言 120pt 宽框换成多行、高度增长到容纳所有行、变短不自动缩回；手动缩到 20pt 后画布只剩首行像素，但文本仍保留在文档与提取中，放大后重新显示。实现方式是把超出高度的行以 Invisible render mode 写入（视觉裁剪、内容与提取保留），不是删除，也不是自动缩小字号。前端输入时用估计行高让编辑框实时增高。
- 勾选 4：`empty_and_filled_boxes_can_be_deleted_and_undone`、`multiple_boxes_keep_independent_content_and_offsets`、`undo_and_redo_restore_exact_box_state_after_saving` 覆盖空框、删除、undo/redo、多框偏移以及保存后 undo/redo 精确恢复；空框提交空文本合法，原文字框清空被拒绝并提示改用删除。
- 未勾选 5：需要真实 GUI 在 macOS Preview 与 Chrome 中打开保存产物做位置/样式视觉比对；自动化已用 PDFium 重新解析保存字节并断言文本/位置/矩阵/字体名，但不等同跨阅读器验收。该项留待发布前人工验收。
- 并发修复审查：`e33f7c3` 的 `open_document` 现在先获取文档 registry 锁，再在锁内打开 PDFium 文档、读取页尺寸与权限；其他所有使用已打开文档的命令本就持有同一把锁。`OPEN_LOCK` 仅在 `open_document` 内、registry 锁之后获取，其他代码不再获取它，因此不存在 registry→OPEN_LOCK 与反向获取的死锁；`EditSession` 在 `OpenDocument` 中声明于 `document` 之前，撤销栈里的游离页面对象先于 PDFium 文档销毁。`text_edit`/`text_boxes` 测试二进制各连续运行 8 轮（每轮并行 4 线程）全部通过，未观察到不稳定。
- 依赖审查：仅新增 `ttf-parser 0.25.1`（`MIT OR Apache-2.0`，crates.io 校验和固定），用于嵌入字体 cmap 覆盖检查，未引入其他 crate。`RUSTSEC-2026-0192` 适用：ttf-parser 已不再维护（informational，无 patched 版本），无内存安全公告；当前仅用于解析文档内已有字体，建议在 ticket 11 字体管线选型时评估替代。`cargo audit` 因本地 advisory-db 含 CVSS 4.0 条目、当前 cargo-audit 0.20.1 不支持而无法完成扫描，属工具限制。
- 自动化验证（合并后于仓库根执行）：`npm ci`、`npm run setup:native`、`npm run typecheck`、`npm test`（13 文件、114 项）、`npm run build`、`npm run test:e2e`（2 项）全部通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 的 4 条 dead_code warning）；`cargo test` 79 项通过、2 项延迟手动测试忽略（`text_edit` 18、`text_boxes` 11、`text_layer` 9 及其他既有二进制）。
- 已知既有问题（与本 ticket 无关，按约定不修）：`cargo clippy --all-targets -- -D warnings` 仍因 `src-tauri/tests/support/fixtures.rs` 的 dead_code 失败。
- 后续建议：`save_document_bytes` 目前调用 `save_to_bytes()`（flags=0，整份重写），ticket 10 应按 ADR-0010 改用带 `FPDF_INCREMENTAL` 的保存入口；`is_win_ansi_char` 接受 `\t` 但 WinAnsi 无 0x09 字形，粘贴制表符可能产生 .notdef；原文字框目前只能在编辑态改文字，移动/缩放/旋转与样式面板仅对新建框开放，若需对齐 spec 用户故事 39/30 应在后续 ticket 评估。
