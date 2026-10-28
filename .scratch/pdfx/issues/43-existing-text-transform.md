# 43: 现有文字的移动、缩放、旋转与整框样式

**What to build:** 在“编辑文字”模式下，现有 Text Object 可以像新 Text Box 一样被选中后移动、缩放、旋转，并整框修改字体、字号、粗体/斜体/下划线、颜色与对齐；不 reflow 周边文字；所有变换可撤销/重做；保存后重新打开时位置与样式一致。

**Blocked by:** 09 (新增 Text Box 与样式), 10 (Save As / Save Changes 管线)

**Status:** done

- [x] 点击现有文字进入可变换的 Text Box，初始几何与样式来自原 Text Object。
- [x] 移动/缩放/旋转后原文字从原位置移除，并以新变换写入，不出现重复文字。
- [x] 整框样式修改立即生效；字体列表只包含内置可自由嵌入字体。
- [x] 变换与样式修改可撤销/重做，保存后仍可撤销到保存前。
- [ ] 保存后重新打开（PDFium 断言；Preview/Chrome 手动验收）位置与样式一致。
- [x] 不 reflow 周围文字；Scanned PDF 上禁用并显示原因。

**Notes:** 票据 08 只覆盖内容编辑，票据 09 只把“新建”的 Text Box 纳入变换/整框样式；现有文字的几何变换与整框样式仍缺失（用户故事 30/39）。实现时应复用票据 09 的 Text Box 几何与样式模型，而不是另建一套。

**Verification notes:**

合并与修复：merge commit `a15a46f`（`ticket/43-existing-text-transform` 分支头 `5072366`，其自身已合入集成分支 `6e95331`；以 `--no-ff` 合入 `integration/pdfx`，集成分支原 tip `1fe2742` 是父提交，无冲突）。独立审计发现并修复一个真实缺陷：现有 Text Object 的字体名来自 PDF 本身（如 `ArialMT`、`Helvetica-Bold` 或内嵌字体名），不在样式面板提供的内置字体列表内；原实现用该名称初始化字体下拉框与整框样式请求，导致下拉框显示为空，且任何整框修改（粗体、颜色、对齐等）都被后端 `TextBoxStyle::from_view` 以 `UnsupportedFont` 拒绝，整框样式对这类现有文字实际不可用。修复提交 `ef40f51` 在面板中当原字体不在列表时改用第一个内置可嵌入字体系列（Helvetica）初始化并发送，新增回归测试（修复前该测试失败，修复后通过）；字体列表本身未扩大。

独立验证的行为：

- 选择与几何：前端 `findCharAt` 用文字层字符的显示空间坐标做命中测试，跳过 generated/零尺寸字符，6pt 容差；后端 `text_edit_target` 用同一文字层的原始字符索引解析到页面 Text Object，几何、旋转与样式（字体名、字号、粗体/斜体、颜色、下划线）取自原对象。`clicking_existing_text_selects_a_listed_box_with_its_original_geometry` 覆盖初始几何与列表登记；前端单元测试覆盖点击、空白处拖动仍为新建。页面旋转时字符坐标已经过 `pointFromEvent`/文字层的显示几何映射；审计探针另在 90/180/270 页面旋转下核对移动增量与请求值相差不超过约 1.3pt（0.8em 基线模型的固有近似），旋转角与文字完整性保持。
- 透明/不可见文字：审计探针用 `3 Tr` 不可见文字构造 PDF，验证它仍可通过字符盒选中（不误选相邻内容），移动后 render mode 保持不可见（移动前后整页均无墨迹）、全文只出现一次，保存重开后仍只有一份且不可见。水印文字在 `target_for` 解析记录之前就被 `is_watermark_object` 拒绝，`watermark_text_is_not_editable_as_a_text_box` 通过；点击水印不会进入可变换 Text Box。
- `text_boxes` 列表：只返回已解析的现有记录（点击后才登记，`created:false`、无 layout/style），保持记录插入顺序，过滤 `deleted` 墓碑；现有记录按物理页面索引存储，页面逻辑顺序变化不会串页。`text_boxes_lists_resolved_strings_in_stable_order_and_hides_deleted_ones` 覆盖顺序、删除与 undo；审计探针覆盖删除页面后旧逻辑页不再列出、undo 页面操作后恢复列出、旋转页面后仍列出且可变换、复制页面不会把记录复制到副本、保存重开的新会话在点击前不列出任何旧记录（旧会话 id 不泄漏）。前端 `onDocumentChanged` 触发 `reloadKey` 重新拉取，选中项在仍被列出时保留；页面操作与 undo/redo 不会让列表暴露已不存在的对象。
- 变换与样式路径：移动/缩放/旋转走既有 `transform_text_box`，整框样式走既有 `apply_text_box_style`；两者与新建 Text Box 共用同一 `TextBoxRecord`/`BoxLayout`/`TextBoxStyle` 模型，没有第二套几何或样式模型。现有文字移动后原文字对象恰好移除一次并以新变换重建，`moving_existing_text_leaves_one_copy_and_does_not_reflow_the_page` 断言全文只有一份且周边文字坐标不变；缩放 `resizing_existing_text_rewraps_without_duplicating_it` 只重排框内文字；旋转 `rotating_existing_text_turns_it_and_survives_saving` 与旋转页面上的 `moving_existing_text_on_a_rotated_page_keeps_its_orientation` 覆盖方向与保存。现有对象使用 `auto_height: true`，避免按文字层较矮的 bbox 裁掉重建后的行；手动缩放的框仍按用户高度裁剪。`legacy_box_rect` 对 90/180/270 度做宽高互换与旋转原点换算，其他角度保留文字编辑覆盖层原有的轴对齐近似（代码注释已说明）。
- 整框样式：`style_change_layout` 以首行基线为锚点物化 canonical layout，改字号时文字不跳动；审计探针在 90/180/270 页面旋转下修改字号与颜色，首行基线保持不变。字体只来自 `text_box_fonts` 的 Base14（Helvetica/Times/Courier × 4 变体）与内置 Noto Sans SC；`fonts::normalize_family` 不做别名映射，未知字体系列被 `TextBoxStyle::from_view` 拒绝。保存后重开由 PDFium 文字层断言字体名、字号、颜色、下划线、粗斜体与位置。
- 撤销/重做：变换与样式都记录为 `BoxEditAction`（分离的页面对象快照 + `RecordSnapshot`，含 `record_index` 以便 redo 重新插入）；`shift_object_indexes` 同步维护 TextBox/Image/Line/Watermark 的范围，插入/删除文字不会错位。`mark_document_saved` 只记录保存点、不清空历史，因此保存后仍能撤销到保存前；`undo_and_redo_existing_text_edits_reach_past_a_save` 覆盖从保存前到保存后多次 undo/redo 与再次保存的字节一致性。保存后重开为新会话，历史为空（既有语义）。
- 只读与扫描页：只读文档在命令边界拒绝 target/transform/style/delete，返回 `PermissionDenied`，`text_edit.rs` 的只读用例覆盖；前端在只读时也不进入编辑。扫描页没有文字层，既有代码以中文原因“此页没有可编辑文字”禁用文字编辑，`PageTextLayer.test.tsx` 覆盖该文案。
- 回归与边界：新建 Text Box 的既有测试（创建、样式、删除、undo、保存重开、字体、自动高度）全部通过；字体会话、保存管线、水印、页面操作套件无回归。没有新增依赖（`Cargo.toml`、`Cargo.lock`、`package.json`、`package-lock.json` 相对 `1fe2742` 未变），无遥测、无网络请求。

门禁（合并与修复提交之后的主仓库）：`npm run typecheck` 通过；`npm test` 51 个文件 / 626 个测试通过；`npm run build` 通过；`npm run test:e2e` 12 个通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` 通过，仅有 4 条 vendored `pdfium-render` 既有 warning；`cargo test` 38 个测试目标，473 通过 / 0 失败 / 3 ignored（`text_boxes` 套件 19 个测试全部通过，其中票据新增 8 个）。测试支持代码的 `dead_code` warning 是仓库既有的按测试二进制裁剪现象，不参与 clippy 门禁。

限制与待办：

- 第 5 项未勾选：PDFium 保存后重开断言已通过（位置、字体、字号、颜色、字形、下划线），但本环境没有执行真实 Preview 与 Chrome 的人工打开验收；该项仍待人工核对。
- 非 90 度旋转的现有文字仍使用文字层轴对齐 bbox 作为初始几何（与既有覆盖层一致，代码注释已说明），不是精确旋转矩形；90/180/270 度已做精确换算。
- 拖拽/缩放手势在 jsdom 单元测试中模拟、命令边界测试覆盖真实 PDF 结果，但没有在真实应用窗口中人工拖动验收。
