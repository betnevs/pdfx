# 35: Flatten 注释导出

**What to build:** 用户可以把视觉 Annotation（高亮、下划线、删除线、便签、手绘、形状、图章、高级注释）Flatten 成页面内容的副本；链接与 Form Field 保持可交互。

**Blocked by:** 12 (文本标记注释与注释列表), 13 (便签与手绘注释), 20 (矩形与椭圆形状), 25 (图章), 34 (高级注释)

**Status:** ready-for-agent

- [ ] 导出副本中视觉 Annotation 变成页面内容，任何阅读器显示一致。
- [ ] 链接与 Form Field 不被 Flatten，仍可点击/填写。
- [ ] 原 Document 不被修改；Flatten 只写新副本。
- [ ] 便签等无页面外观的注释有明确处理（渲染图标或省略）。
- [ ] 导出结果在 Preview、Adobe Acrobat Reader、Chrome 中验证。

**Verification (agent):** 分支 `ticket/35-flatten-export`，功能提交 `1a35297`；先合入 integration tip `b8a1be2`（仅 `.scratch/pdfx/spec.md` 的文档改动，合并提交 `29c4ee9`，无冲突），再在合并后的树上跑完整门禁。相对 integration tip 的代码改动为 12 个文件（+2894/−212）：`flatten_export.rs`、`atomic_write.rs`、`save.rs`、`print_export.rs`、`commands.rs`、`lib.rs`、`tests/flatten_export.rs`、`PrintExport.tsx`、`PrintExport.test.tsx`、`backend.ts`、`backend.test.ts`、`styles.css`；未改 `src-tauri/vendor`（`PDFX-PATCH.md` 仍为 15 文件 / 10 个 close 类型，`git diff` 为空，无新 hunk）、`Cargo.toml`/`Cargo.lock`/`package.json`/`package-lock.json`、spec、ADR、`tauri.conf` 或 capabilities，未新增依赖，无网络/遥测。

**实现：** 新命令 `export_flattened_copy(document_id, destination, confirm_unencrypted_output)` 通过统一命令边界测试。命令先拒绝空目标与 Source/当前 Output（复用 `target_path::same_file`，覆盖符号链接与大小写变体），再用新提取的 `save::serialize_live_document` 把当前 live 状态（含未保存编辑、表单与对象编辑）序列化为内存字节，全程不触碰 PDFium 文档、历史或 Source；随后 `flatten_document` 用 lopdf 解析，仅对含视觉注释的页克隆页面并追加一个 lopdf 增量 revision：把每个注释可用的 `/AP /N` Form XObject 加入页面（含继承）`/Resources /XObject`（唯一 `PDFXFlatN` 名），按 BBox/Matrix→Rect 映射写 `cm` 与 `q … Do Q` 到单个内容流，并从该副本的 `/Annots` 移除已绘制注释；最后用共享的 `atomic_write::write_atomically` 写同目录临时文件、`sync_all`、`rename`，失败不留半成品（`print_export` 也改用该 helper，删掉重复实现）。原页面 `/Contents` 支持缺失、单流、数组、直接对象与 Null，未改动页保持原内容流字节一致；页面 `/Rotate` 无需额外变换（注释与页面内容共用未旋转用户空间，旋转由阅读器统一应用），已由 90/180/270 渲染用例验证。分类策略：`/Link`、`/Widget`（含签名字段 widget）原样保留并计入 `preserved`；`/Popup`、隐藏/不可见/NoView 注释、无可放置外观的注释（无 Form AP、图像 AP、BBox 无效）被省略并逐条返回 `skipped`（`popup`/`hidden`/`no_appearance`/`unsupported_appearance`），不会留下可能损坏副本的残留；PDFX 便签自带图标 AP，因此按普通视觉注释 flatten；若注释带有关联 `/Popup`，该 Popup 因无页面外观而省略。加密 Source 沿用 ADR-0007：未显式确认时返回既有 `unencrypted_output_confirmation_required` 且不写文件，确认后只写未加密副本、源文件字节与 mtime 不变。

**自动化验证（合并 integration tip 后执行）：** `npm run typecheck` 通过；`npm test` **37 文件 / 427 项**全部通过（本票净增 9 项前端用例：`backend.test.ts` 3 项命令/错误码映射，`PrintExport.test.tsx` 6 项覆盖 flatten 入口、成功摘要、保存对话框取消、Source/Output 中文保护提示、加密未确认门禁与可重试失败）；`npm run build` 通过；`npm run test:e2e` **12 项**通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 既有 4 条 warning，未用 `--all-targets`）；`cargo test` **298 通过 / 0 失败 / 3 ignored**，其中新增 `tests/flatten_export.rs` **10 项**全部通过（较 integration tip 净增 10 项 Rust 用例）。该 suite 覆盖：全部 PDFX 注释类型（高亮、下划线、删除线、便签、手绘、矩形、椭圆、图章、波浪线、自由文本、callout）flatten 后无视觉注释残留、内容流出现 `PDFXFlat* Do`、PDFium 重开为 0 个注释且渲染像素与 live 渲染 MAE < 2.0；URI 链接与 AcroForm 文本字段/12 个 widget 保留、填写后重开仍可继续填写；签名 widget 保留 `/FT /Sig`、无 `/V`、AcroForm 不引入 `/NeedAppearances`；无外观/隐藏/Popup/图像 AP/无效 BBox 注释的跳过原因顺序；旋转页；无注释页内容流字节不变；Source/Output/live 文档与撤销历史不被改动（含符号链接别名拒绝）；重复导出覆盖同一目标、缺失父目录与目录目标失败不留文件；加密源未确认拒绝/确认后未加密副本可重开；书签、元数据与既有链接保留。

**人工未验项（未勾选）：** 导出副本在 Preview、Adobe Acrobat Reader、Chrome 中的真实阅读器矩阵（对应勾选项 5）；真实 Tauri 窗口中的保存对话框与中文提示手感。自动化只覆盖 PDFium 渲染/重开、独立 lopdf 结构、命令边界与 jsdom 前端交互。

**已知限制/后续：** 不栅格化，完全以注释自身 `/AP /N` 为准；无可用 Form AP 的外来视觉注释按报告策略省略而非绘制占位；隐藏注释与 Popup 不写入副本；`same_file` 只检测同一路径、符号链接与大小写变体，硬链接别名不会被拒绝；签名字段仅保留结构，不执行证书签名；加密源确认后副本不加密。

