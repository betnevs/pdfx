# 14: 插入图片

**What to build:** 用户可以插入 PNG/JPG 图片，调整大小与旋转，保存后图片位于正确 Page 与位置。

**Blocked by:** 10 (Save As / Save Changes 管线)

**Status:** done

- [x] 文件对话框选择 PNG/JPG 后插入当前 Page，默认保持宽高比。
- [x] 图片可移动、缩放、旋转；超出 Page 边界时有可见提示或裁剪策略。
- [ ] 保存后图片在 Preview、Chrome 中位置与尺寸一致。
- [x] 撤销/重做覆盖插入、移动、缩放、旋转与删除。

**Verification notes:**

- 合并提交 `4381757`（`git merge --no-ff ticket/14-image-insertion`）；分支合并提交 `3882337`（把 integration tip `76ff868` 合入分支 `6015b02`，feature `959cadf`）。合并基为 `da62ed7`（ticket 29 合并之后），integration 侧已含 ticket 12（文本标记注释）与 ticket 11（CJK 字体）。审计补充提交 `fc81ccd`（混合图片/注释统一历史测试 + `page_images` 文档修正）。合并后未修改 spec、ADR、`save.rs`、打包配置或其他 ticket。
- 冲突与解决：`src-tauri/src/lib.rs`（保留 `mod fonts` + `pub mod image_edit`，并同时注册 image 与 annotation 命令）；`src-tauri/src/text_edit.rs`（`EditAction` 合并为 `Box`/`Image`/`Annotation` 三支；`undo_for`/`redo_for` 采用 integration 的“先看栈顶再弹出”权限分派，Box/Image 要求内容权限、Annotation 要求注释权限；Box 路径全部改用 `shift_object_indexes`，使图片与 Text Box 共享同一页面对象索引表）；`src/lib/backend.ts`（图片视图类型与注释视图类型并集）。预期中的 `save.rs`、`App.tsx`、`Cargo.toml`/`Cargo.lock` 无冲突：`save.rs` 未改，图片依赖（`png` 0.18、`image` 0.25 的 `jpeg` feature）在 ticket 27 已存在，相对合并基无依赖变化。`src/App.test.tsx`、`PageTextLayer.tsx`、`PageViewer.tsx`、`styles.css` 自动合并，保留双方功能。
- 审计修复（并入 `3882337`）：`documentEvents.onDocumentChanged` 原先重建 detail 时丢弃了 `source`，导致图片动作后文本框工具条的共享撤销可用性不刷新（图片插入后“撤销”按钮与 Cmd/Ctrl+Z 仍不可用）。现仅在 `source` 为 `"text"`/`"image"` 时透传，并新增 `src/lib/documentEvents.test.ts` 覆盖透传、未知 source 过滤与取消订阅。`fc81ccd` 另修正 `page_images` 的 TS 注释：它只列出本会话插入的图片，不包含 Source Document 中已有的图片。
- 勾选 1：`chooseImageFile` 使用原生对话框，过滤器名称“图片”、扩展名 `png`/`jpg`/`jpeg`；`insert_image` 在命令边界完成全部校验后才触碰文档。`inserting_a_png_places_a_centered_aspect_fitted_image_on_the_current_page` 断言 200×100 图片以自然尺寸（不超过页宽/页高 80%）居中于 612×792 页面：x=206、y=346；`a_large_image_is_scaled_down_to_fit_inside_the_page` 断言 2000×500 按 0.2448 等比缩放为 489.6×122.4 且完整落在页面内；`a_jpeg_is_accepted_and_placed_on_the_page` 覆盖 JPEG。前端 `PageImageLayer.test.tsx` 覆盖选择文件→调用命令→覆盖层按百分比定位。真实 Tauri 窗口中的系统文件对话框未手动点击验证。
- 勾选 2：移动（拖动轮廓）、缩放（右下角手柄，保持宽高比、中心固定）、旋转（旋转手柄，绕中心，Shift 15° 吸附）在 `imageGeometry.ts` 中实现并有 8 项单测；`PageImageLayer.test.tsx` 断言移动/缩放/旋转各只提交一次 `transform_image` 且覆盖层位置/角度更新。超出 Page 时的策略为“可见提示 + 页面裁切”：覆盖层 `overflow: hidden` 并显示中文 `图片超出页面，超出部分将被裁切。`（`role="status"`，旋转后的四角越界检测，含 0.01pt 容差），保存时保留完整图片几何、由 PDF 页面在渲染时裁切，移回页内即恢复被裁部分，源图片文件从不被裁切或改写。`a_saved_image_reopens_with_its_position_size_and_rotation` 断言保存后 PDFium 重开在变换后中心渲染出图片、旋转矩形外保持白色；`a_saved_image_stays_on_its_page_and_rotated_pages_use_display_geometry` 用 90° 旋转 + CropBox fixture 证明插入使用显示几何（600×400 显示空间，x=200、y=150），且其他页面不受影响。
- 勾选 3 **未验证（人工）**：保存路径自动化证据充分——`a_saved_image_reopens_with_its_position_size_and_rotation` 通过 ticket 10 的 `save_document_with_progress` 保存后：(a) 用独立 `lopdf` 解析确认页面仅 1 个图片 XObject、像素尺寸 200×100、内容流 `cm` 矩阵与显示空间旋转 30° 的期望矩阵逐项误差 <0.2；(b) 通过命令边界重新打开输出文件，PDFium 渲染在变换中心出图、矩形外为白；(c) 断言 Source Document 字节完全未变。`undo_after_save_removes_the_image_from_a_later_save` 证明保存后撤销再 Save Changes 会写出不含图片的新 revision。但 Preview、Chrome（以及 Acrobat/Edge）中的实际位置与尺寸未在本机手动打开验证，故保持未勾选，留给跨阅读器人工矩阵。
- 勾选 4：图片动作进入与 Text Box、Annotation 相同的 `EditSession` 撤销/重做栈，`ImageEditAction` 保存操作前后的页面对象快照与记录快照，插入/变换/删除各为一步。`undo_removes_a_new_image_and_redo_restores_it`、`moving_resizing_and_rotating_an_image_is_one_undoable_action`、`deleting_an_image_removes_it_and_undo_restores_the_page` 覆盖插入、移动/缩放/旋转、删除的撤销与重做（含像素断言）；`image_actions_keep_text_box_object_ranges_in_sync` 覆盖图片对象增删后 Text Box 记录索引随动（删除图片→撤销→继续编辑 Text Box 仍命中原对象）；`undo_after_save_removes_the_image_from_a_later_save` 覆盖保存不切断历史、dirty 基线随保存移动；`image_and_annotation_actions_share_one_undo_history`（`fc81ccd`）交错插入图片与高亮注释，按最新优先撤销/重做，证明两种工具共用一条历史且互不泄漏。前端在图片动作后通过 `notifyDocumentChanged({ source: "image" })` 刷新共享撤销状态（见审计修复）。Form Field 动作尚未并入 integration（ticket 30 仍为独立分支），其并入时需加入同一 `EditAction` 联合；本 ticket 已把索引维护抽成 `shift_object_indexes` 供内容对象类动作共用。
- 校验与错误：仅接受 PNG 与 JPEG；扩展名（大小写不敏感）必须与文件魔数一致，扩展名或魔数不符返回 `unsupported_image_format`，损坏/无法解码返回 `image_decode_failed`，文件 >25 MiB 或像素 >20,000,000 返回 `image_too_large`。25 MiB 限制先用 metadata 拒绝，再以 `take(MAX+1)` 读取以防文件在检查后增长；20 MP 限制在 PNG/JPEG 头解析后、分配像素缓冲前检查（`oversized_dimensions_png` 用伪造 IHDR + 正确 CRC 验证）。`invalid_image_files_are_rejected_without_changing_the_document` 覆盖错误扩展名、扩展名/魔数不符、损坏 PNG/JPEG、超大尺寸、>25 MiB 稀疏文件，并断言文档无图片、历史不 dirty、页面像素未变；失败插入不会留下半成品编辑。后端错误信息为英文（沿用既有命令边界约定），前端按错误码映射为中文（`仅支持 PNG 和 JPG 格式的图片。`、`无法读取这张图片…`、`图片过大：文件不能超过 25 MB，像素不能超过 2000 万。` 等），`PageImageLayer.test.tsx` 断言中文错误与不生成覆盖层。
- 保存写入者/保真：图片是真正的 PDFium `PdfPageImageObject`（BGRA bitmap + `apply_matrix` 单位方阵映射），与 Text Box 一样由 `FPDF_SaveAsCopy(FPDF_INCREMENTAL)` 写入；`image_edit.rs` 不包含任何保存路径，也不写 lopdf 对象，保存唯一走 ticket 10 管线。插入/变换只追加或替换图片页面对象，不触碰既有文字对象，无 reflow；`image_actions_keep_text_box_object_ranges_in_sync` 在图片操作后仍能提取到 `Original line.` 与编辑后的文字。
- 只读/权限：`insert_image`/`transform_image`/`delete_image` 在修改前调用 `ensure_content_editable`，受限文档返回 `permission_denied` 且不 dirty；`page_images` 作为读操作仍可用。`read_only_documents_reject_image_editing` 用 encrypted-owner-restrictions fixture 覆盖三者；前端在 `readOnly` 时禁用插入/删除按钮并显示“此文档不允许编辑内容”（`PageImageLayer.test.tsx` 断言不打开对话框）。注意：`insert_image` 会先解码文件再检查文档权限（不修改文档，仅浪费一次解码）。
- 隐私/依赖/文案：新增代码只读本地图片文件，无网络、遥测、上传或日志写入图片内容；源图片字节以 `Arc<Vec<u8>>` 保存在会话内用于撤销后变换重建，从不回写源路径。相对合并基无新增 crate/包（PNG 用既有 `png`，JPEG 用既有 `image` 的 `jpeg` feature），无新许可证或 advisory。新增 UI 文案与对话框过滤器均为中文。
- 单一命令 seam：新增 4 个 Tauri 命令 `insert_image`、`page_images`、`transform_image`、`delete_image`；前端仅经 `src/lib/backend.ts` 调用，引擎类型不越过命令边界。Rust 验收测试全部通过命令函数与固定 fixture/临时图片驱动，未直连 PDFium/lopdf 做编辑。
- 自动化验证（合并后在仓库根执行）：`npm run typecheck` 通过；`npm test` 25 文件 248 项全部通过（新增 `PageImageLayer` 8、`imageGeometry` 8、`documentEvents` 3，`App` 增加 `page_images` 默认桩）；`npm run build` 通过；`npm run test:e2e` 12 项通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 4 条既有 dead_code warning，符合预期）；`cargo test` 180 项通过、0 失败、3 项 ignored（既有手动/延迟测试），其中 `image_insertion` 13 项（原 12 + `fc81ccd` 的混合历史 1 项）。
- 已知限制/后续：
  - ticket 23（图像签名）：可复用 `image_edit` 的解码/校验（PNG alpha 已进入 BGRA 并通过 `PdfBitmapFormat::BGRA` 保留）、默认居中 fit 与统一撤销；但 `insert_image` 当前只接受文件路径。绘制/输入签名产生内存位图，建议新增“内存字节/来源枚举”的插入命令（勿写临时文件绕过 seam），并补一张透明 PNG 保存重开的像素测试（当前无 alpha 专项断言）。签名命名与来源（`name` 字段）可直接复用 `ImageRecord`。
  - ticket 24（超链接）：`page_images` 与 `ImageObjectView.imageId` 只覆盖本会话插入的图片；若要给 Source Document 中已有图片加链接，需要扩展图片身份/列表接口（目前既有图片不可选中）。链接是原生注释，建议沿用 ticket 12 的注释模型加入同一 `EditAction` 联合（注释权限、无页面对象索引移动），图片矩形可直接取自 `page_images` 的显示空间几何；`shift_object_indexes` 必须继续只维护内容对象记录。
  - ticket 30（AcroForm 填表）并入时：`EditAction` 需新增字段动作分支并补齐 `undo_for`/`redo_for` 的权限分派；表单字段是注释/widget 对象，不应触发 `shift_object_indexes`。合并后应重跑 `image_insertion` 的索引同步与混合历史测试。
  - 展示层：图片工具条固定在每页左下角，撤销/重做仍在共享的文本工具条（图片工具条不重复提供）；若某页文字层载入失败（`PageTextLayer` 错误分支不渲染工具条），该页暂时没有可点的撤销入口，这是既有共享工具条布局的限制。
  - 性能：每次变换提交都从原始字节重新解码图片（20 MP JPEG 最坏约数百毫秒）；如需更顺滑的连续变换，可在会话内缓存解码结果（会增加内存常驻）。
  - 未验证的人工 GUI 项：真实 Tauri 窗口中的系统文件对话框、指针拖动/缩放/旋转手感、透明 PNG 的视觉表现，以及勾选 3 的 Preview/Chrome/Acrobat/Edge 跨阅读器矩阵。自动化仅通过 mock IPC、命令边界测试、PDFium 重开与像素断言覆盖。
