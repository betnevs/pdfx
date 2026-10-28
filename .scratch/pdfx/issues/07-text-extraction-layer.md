# 07: 文本提取与文本层

**What to build:** 每页渲染出与底图对齐的文本层，用户可以点击与选择文字；Scanned PDF 或没有文本层的 Page 显示“此页没有可编辑文字”的提示。

**Blocked by:** 04 (打开文档并渲染第一页)

**Status:** done

- [x] 文本层字符位置与渲染底图对齐，缩放/旋转后仍对齐。
- [x] 可以按字符、词、行选择文字；选择结果能驱动高亮与复制。
- [x] 文本型、CJK、混排、CID/Type0 字体的 Page 都能提取到文字。
- [x] Scanned PDF 不产生伪文本层，并显示不可编辑提示。
- [x] 文本提取不阻塞翻页与缩放；大文档按页懒加载。

**Verification notes:**

- 合并提交 `98a9ea4`（ticket/07-text-extraction `7768a08`），父提交为 ticket 06 关闭后的 `15e892d`。冲突：`src/App.tsx`（保留 #05 的 `useDocumentViewer`/`DocumentToolbar`/`ThumbnailRail`/`PageSurface` 结构与 #06 的密码/`DocumentAccess` 流程，把文本层下沉到 `PageSurface` 而不是 App）、`src/App.test.tsx`（同时保留 #05 的 `defaultPageInfo`/缩略图/区域 handler 与 #07 的 `textLayerView`/`extract_text_layer`，两个测试均保留）、`src/lib/backend.test.ts`（#05 的分页/缩略图/区域测试与 #07 的文本层测试并存）、`src/styles.css`（保留 #05 的 `.page-image`/`.page-region` 与 #07 的全部 `.page-text-layer` 规则，不再保留已删除的 `.page-canvas`）、`src-tauri/src/commands.rs`（PDFium prelude import 取并集）。`backend.ts`、`lib.rs`、`PageTextLayer.tsx`、`textLayer.ts`、`tests/text_layer.rs`、`tests/support/synthetic.rs` 自动合并。未复活旧单体 App，导航/缩略图/全屏与密码流程仍在。
- `PageTextLayer` 作为 `PageSurface` 内 `.page-surface` 的子元素渲染，页面表面始终是当前缩放下的整页像素尺寸；文本几何按 `pageWidth`/`pageHeight` 百分比定位，因此 4000% 下的视口分块（`render_page_region`）与文本覆盖层共用同一页面坐标系，覆盖层不会缩到单个 tile。以 `documentId:pageIndex` 作为 key 保证换文档/换页时重挂载，组件内部仍保留请求取消与 stale response 守卫，一次只提取当前可见页。
- `lib.rs` 已注册 `page_info`、`render_thumbnail`、`render_page_region`、`extract_text_layer`，`open_document` 仍接收可选 `password`；`backend.ts` 同时导出 `PageInfo`/`PageRegionView`/`TextCharView`/`TextLayerView`、`DocumentPermissions`/`encrypted`/`readOnly` 与对应调用。
- 自动验证通过：`npm run typecheck`、`npm test`（8 个文件、88 项）、`npm run build`、`npm run test:e2e`（2 项）、`cargo fmt --check`、`cargo clippy -- -D warnings`、`cargo test`（50 项通过，2 项手动延迟测试默认忽略）。Rust `tests/text_layer.rs` 覆盖 Unicode 字符框、与渲染墨迹对齐、旋转页对齐、CJK/拉丁混排、Type0/CIDFontType2、代理对、无文本页空层、CropBox 偏移与旋转 CropBox 映射；前端覆盖按页懒加载、scanned 提示、换页 stale 丢弃、缩放时百分比高亮几何、拖选/双击选词/三击选行、按钮与键盘复制、Escape 清除。
- 未做人工 GUI 验证：真实桌面 WebView2 中的鼠标拖选/双击/三击、系统剪贴板复制、CJK 字体实际渲染与 IME 交互、真实 scanned PDF，以及 4000% 分块渲染下文本层与底图的像素级对齐，目前仅由 jsdom 组件测试与 Rust 几何测试覆盖，需桌面端冒烟。
- 残余集成风险：`PageSurface` 在 #05 中以 `role="img"` 暴露“第 N 页”可访问名称，文本层是嵌套其中的可聚焦 `role="region"`；jsdom 中两者都能查询到，但 ARIA 的 presentational children 语义可能让部分屏幕阅读器忽略可交互文本层，需要一次可访问性手工检查。本次未改动 #05 的 `role="img"` 契约。
- 既有问题（与本 ticket 无关，按约定未修复）：`cargo clippy --all-targets -- -D warnings` 除 `tests/support/fixtures.rs`/`tests/fixtures.rs` 的 dead_code 外，现在也因 #07 新增的 `tests/support/synthetic.rs` 测试辅助代码 dead_code 失败；这是同一类“每个集成测试 crate 只使用部分辅助函数”的既有模式，普通 `cargo clippy -- -D warnings` 通过。
