# 46: 打印与导出的健壮性

**What to build:** 打印大文档时不再把所有页面位图同时解码进内存，导出图片采用原子写入，并且导出目标不能覆盖 Source Document 或当前 Output Document。

**Blocked by:** 27 (打印与导出当前页)

**Status:** done

- [x] 打印按页或分块准备/解码位图，内存占用有上限；2000 页全范围打印不会一次性解码所有页面。
- [x] 打印取消会释放已准备的位图与临时状态，不残留 DOM/内存。
- [x] PNG/JPG 导出先写同目录临时文件再原子重命名；磁盘满或中断不会在目标处留下半张图片。
- [x] 导出目标与 Source Document、当前 Output Document（含符号链接/大小写变体）相同时拒绝，并显示明确中文原因。
- [x] 大文档打印与导出有可验证的进度/取消行为；测试覆盖内存上限路径与目标保护。

**Notes:** ticket 27 的审计发现：打印表面把每一页 150 DPI PNG 以 base64 保存在 print DOM，并用 `Promise.all` 同时解码（Letter 每页 RGBA 约 8.4MB，2000 页可能达数十 GB）；导出使用非原子 `fs::write`；导出只保护 Source Document，选当前 Output Document 路径会把已保存的 PDF 替换成图片字节（仅靠系统覆盖确认）。本票只做健壮性修复，不改变 ticket 27 的交互与快捷键；菜单项置灰属 ticket 45。

**Verification notes:**

- 合并：分支基于 `19f9fcb`，实现提交 `4ea8355`（10 文件，+862/−110，含新增 `src-tauri/src/target_path.rs`）；随后 `git merge --no-ff integration/pdfx` 合入 tip `24e23ff`（ticket 13 便签/手绘），得到合并提交 `f9f29d7`，**无冲突**（`lib.rs`、`backend.ts` 由 git 自动合并；ticket 13 的命令注册与类型扩展、本票的模块/错误码并存）。未修改 spec、ADR、依赖、`tauri.conf.json`、capabilities 或打包配置。合并后工作树干净。
- 勾选 1（打印内存上限）：前端打印准备改为严格串行——每页 `render_page` + `page_info` → 生成 PNG Blob 的 object URL（不再把 base64 放进 print DOM）→ 用 `HTMLImageElement.decode()` 解码后再处理下一页。峰值解码 RGBA 固定为 1 页（Letter 150 DPI 约 8.4MB），与所选页数无关；打印 DOM 只保留每页的 object URL 字符串，压缩 PNG 由 WebView 的 blob store 持有（可落盘），JS 堆不持有位图数据。`prepares and decodes a full large print range one page at a time` 用 2000 页文档断言 `maxLive === 1`（同时解码的位图数）、`rendersAtFirstDecode < 2000`（边渲染边解码而不是先堆积全部）、2000 个 `blob:` URL、print DOM `innerHTML` 不含 `base64`，并最终调用 `print_document`。ticket 27 的打印选项（范围/方向/缩放）、快捷键与命令集合保持不变。限制：自动化在 jsdom + mock 命令下验证请求/解码模式，真实 2000 页文档在桌面 WebView 中的实际内存曲线未人工 profile；压缩 PNG Blob 在打印结束前仍全部存活（对象 URL 由 WebView 管理，非 JS 位图）。
- 勾选 2（取消清理）：准备循环持有 generation token，取消、换文档、组件卸载都会递增 token 并 `revokeObjectURL` 所有已准备 URL、清空 `printPages`/`printSize`/进度与对话框；stale 循环在 `render_page`/`decode` 返回后发现 token 变化即退出，既不 `print_document`，也不会释放新任务的位图。`releases prepared print bitmaps when preparation is cancelled` 断言取消时 2 个已创建 URL 全部 revoke、关闭对话框、放行挂起渲染后不再创建 URL/不打印/无 `.print-root`；`releases prepared print bitmaps when the component unmounts mid-preparation` 断言卸载同样 revoke 全部 URL 且不打印；既有 `shows preparation progress for a large document and can cancel it` 继续覆盖 200 页进度与取消（取消时尚未创建 URL）。`prepares one print page...` 在 `afterprint` 后断言 revoked 集合等于 created 集合（3/3）且 print DOM 清空。
- 勾选 3（原子导出）：`print_export.rs` 的 `write_image_atomically` 在目标同目录创建 `.<name>.pdfx-export-<pid>-<n>.tmp`（`create_new` 防覆盖），`write_all` 后 `sync_all`，再 `rename` 覆盖目标；写失败、fsync 失败或 rename 失败都删除临时文件并返回结构化错误；成功后在 Unix 上 best-effort `fsync` 目录（Windows 无目录句柄，与保存管线一致）。单元测试 `a_failed_write_leaves_no_target_and_no_temporary_file`、`a_failed_overwrite_keeps_the_previous_target_byte_identical` 通过注入“写了部分字节后 ENOSPC”分别断言目标不存在/旧目标字节不变且无临时文件；`a_successful_write_replaces_the_target_and_leaves_no_temporary_file` 断言成功路径无临时文件。集成测试 `export_write_failure_leaves_the_previous_target_and_no_temp_file` 用只读目录触发真实创建失败，断言旧目标字节不变、目录中只剩 `page.png`、错误码为 `permission_denied` 或 `file_write_failed`。原有 PNG/JPEG 字节与画布一致性、只读导出、DPI/越界校验测试全部保留并通过。进程被强杀时可能残留一个 `.tmp`，但目标仍是旧文件（rename 前不会动目标）。
- 勾选 4（目标保护）：新增 `src-tauri/src/target_path.rs`，把原先 `save.rs` 的规范化比较抽为共享 `same_file`：`canonicalize` 解析符号链接与 `..`，目标不存在时规范化父目录再拼接文件名，macOS/Windows 上小写化以覆盖大小写变体；`save.rs` 改为调用同一实现，避免两套路径语义。导出在 `with_open_document` 内、渲染前检查目标：命中 Source Document 返回新错误码 `export_target_is_source`，命中 `SaveState.output_path`（Save As 后的当前 Output Document）返回 `export_target_is_output`，消息说明是哪个文档。集成测试覆盖源 PDF、Output Document、两者的符号链接别名，以及 macOS/Windows 上的大小写变体（`SOURCE.PDF`/`OUTPUT.PDF`），均断言拒绝且两个文件字节不变；前端映射为中文“不能覆盖源 PDF 文件，请选择其他位置。”与“不能覆盖当前输出 PDF 文件，请选择其他位置。”，`PrintExport.test.tsx` 与 `backend.test.ts` 覆盖新错误码识别与 `role="alert"` 中文提示；原 `invalid_argument` 不再承担“覆盖源文件”语义，改为通用“导出参数无效”。
- 勾选 5（进度/取消/测试覆盖）：打印进度（“正在准备第 N / M 页…” + `<progress>` + “取消打印”）保留，200 页进度/取消与 2000 页内存上限路径均有测试；导出为单页操作，没有伪进度条，其可验证性由原子写与失败清理测试覆盖；目标保护由上述 4 个 Rust 集成测试与 3 个前端/backend 测试覆盖。导出仍是 `export_page(document_id, page_index, format, dpi, destination)`，未新增命令（只新增两个错误码），符合单一命令 seam。
- 隐私/依赖/文案：无新增 `Cargo.toml`/`package.json` 依赖，无网络、遥测或上传调用；日志字段沿用 ticket 27（`document.export_page` / `document.export_page.failed` / `document.print`），不记录页面内容；新增 UI 文案为中文。
- 自动化验证（合并后在 ticket 树、仓库根执行）：`npm run typecheck` 通过；`npm test` **29 文件 / 304 项**通过；`npm run build` 通过；`npm run test:e2e` **12 项**通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 4 条既有 dead_code warning，符合预期）；`cargo test` **221 通过 / 0 失败 / 3 ignored（24 个 suite，含 doc-tests）**，其中 `tests/print_export.rs` 11 项、`print_export::tests` 原子写 3 项全部通过。
- 人工未验项：真实 macOS/Windows 打印对话框与打印机输出（ticket 27 的勾选 1 仍未人工验证），本票改动后的 blob 打印面在真实 WebView 中的分页/首屏空白表现；真实 2000 页文档的内存曲线；真实原生保存对话框中选择符号链接/大小写变体路径的端到端流程；真实磁盘写满时的系统行为（自动化用注入错误与只读目录模拟）。
- 已知限制/带入后续：macOS 上原生打印面板是否派发 `afterprint` 仍未验证（ticket 27 遗留）；若不派发，压缩 PNG Blob 会保留到下一次打印或切换文档，但解码 RGBA 不会被保留。导出仍是单页；ticket 35（Flatten 副本导出）等后续导出功能应复用 `target_path::same_file` 与原子写模式，并同样保护 Output Document。
