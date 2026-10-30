# 08: 编辑已有文字 + 撤销/重做

**What to build:** 用户点击已有文字进入 Text Box 编辑，保留原 Text Object 的字体、字号、颜色与位置；修改后移除原文字并写入新文字，支持删除 Text Box 与统一撤销/重做。

**Blocked by:** 07 (文本提取与文本层)

**Status:** done

- [x] 点击文字进入编辑态，Text Box 的初始样式与位置来自原 Text Object。
- [x] 修改文字后画布立即反映；删除 Text Box 时原文字真正移除，复制粘贴不出现重复文字。
- [x] 不做段落 reflow；超长文字按 Text Box 宽度换行并向下扩展。
- [x] 所有文本编辑动作可撤销/重做，保存后仍可撤销到保存前。
- [x] Scanned PDF 上禁用文字编辑并显示原因。

**Verification notes:**

- 合并提交 `1b38dcb`（ticket/09-text-boxes `e33f7c3`，包含 ticket 08 的 `1fea185`，经 `c3051ef` 合入 integration）；按约定只合并 `ticket/09-text-boxes` 一次（`git merge --no-ff`），无冲突，未单独合并 ticket 08。未修改 spec、ADR 或其他 ticket。
- 勾选 1：`text_edit_target` 按点击字符解析到所属 PDFium Text Object，返回原字体名、字号、颜色、文本与文本层几何。`tests/text_edit.rs::clicking_text_prefills_the_original_style_content_and_position` 断言 Helvetica 18pt、`#000000`、user-space x=72 及覆盖点击字形；`characters_resolve_to_their_own_text_object` 断言同一 Text Object 共享 Text Box、不同行各自独立。前端 `PageTextEditing.test.tsx` 覆盖点击进入编辑态与预填。
- 勾选 2：`apply_text_edit` 先移除原页面对象再插入替换对象；`deleting_a_text_box_removes_the_original_text_everywhere` 与 `applying_an_edit_replaces_the_original_text_everywhere` 断言提取文本无原文残留、新文本只出现一次，并重新解析 `save_document_bytes` 的字节确认不是内存假象。画布即时性由 `the_canvas_reflects_the_edit_at_the_original_position` 的像素断言 + 前端 `documentEvents` 刷新覆盖。
- 勾选 3：`long_text_wraps_at_the_text_box_width_and_expands_downward` 断言按框宽换行、行距向下扩展，未编辑 Text Object 的原点不变（无段落 reflow）。
- 勾选 4：`undo_and_redo_restore_the_exact_text_and_pixels` 断言撤销/重做恢复精确文本与逐像素渲染、分支编辑清空 redo；`raw` 快照保存原页面对象，undo 不依赖重新排版。`history_survives_saving_and_dirty_tracks_the_saved_state` 断言保存后撤销栈保留、可撤销到保存前，`mark_document_saved` 只移动干净基线。保存命令本身属 ticket 10；本 ticket 提供并验证了 `save_document_bytes`/`mark_document_saved` seam。
- 勾选 5：后端 `NoEditableText` 错误带原因（`scanned_pages_have_no_editable_text`）；前端在无文本层时显示“此页没有可编辑文字”并禁用“编辑文字/添加文本框”。只读文档另有 `PermissionDenied` 与工具栏原因，`read_only_documents_reject_text_editing` 覆盖。
- 字体：`decide_font` 保留可覆盖新文本的 Embedded Font（ttf-parser cmap 覆盖检查，`embedded_fonts_are_preserved...`），缺字形或字体无法检查时回退 Helvetica 并返回 `FontUsageView.reason`（`non_standard_fonts_fall_back...`）；非 WinAnsi 文本返回 `UnsupportedFont` 而不是写入 .notdef（`unsupported_scripts_are_rejected...`）。
- 未验证的 GUI 项：真实应用内的连续输入/IME 组字、撤销后焦点恢复，以及 Preview/Chrome 中的视觉保存对比（需人工跨阅读器验收）。自动化仅覆盖命令边界与 jsdom 组件行为。
- 遗留问题（不阻塞）：`is_win_ansi_char` 接受 `\t`，但 WinAnsiEncoding 在码位 0x09 无字形；从剪贴板粘贴制表符可能产生 .notdef，建议后续把 tab 归一化为空格或拒绝。`save_document_bytes` 目前走 `save_to_bytes()`（flags=0，整份重写），ticket 10 应按 ADR-0010 切换到增量保存入口。
