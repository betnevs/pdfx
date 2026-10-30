# 05: 翻页、缩放与缩略图

**What to build:** 用户可以在 Document 内翻页、跳转、缩放与全屏查看；左侧缩略图栏懒加载并可点击定位，底部显示当前页/总页数与缩放比例。

**Blocked by:** 04 (打开文档并渲染第一页)

**Status:** done

- [x] 页码输入、上一页/下一页、Home/End 跳转正确。
- [x] 10%–4000% 缩放、适合宽度、适合页面均可用；缩放后文本层与覆盖层对齐。
- [x] 缩略图栏只渲染可见与邻近页，滚动长文档不阻塞；点击缩略图跳转到对应 Page。
- [x] 全屏查看模式可进入/退出，退出后恢复原缩放与页码。
- [x] 旋转页与混合页面尺寸显示正确，不出现裁剪或错位。

**Verification notes:**

- 合并提交 `fe527c0`（ticket/05-navigation-zoom-thumbnails `8263161`），`--no-ff`，无冲突；仅 `src-tauri/src/lib.rs` 自动合并（保留 ticket 41 的 `pdfium_adapter` 声明）。合并后追加 `e920803` 仅做 rustfmt 格式化，无行为变化。`backend.ts` 的缩放常量改为从 `pageGeometry.ts` 再导出，保留 `DEFAULT_SCALE`/`MIN_SCALE`/`MAX_SCALE`/`ZOOM_STEP` 的既有 seam；缩放范围由 25%–400% 扩为 10%–4000%，Rust `MIN_SCALE`/`MAX_SCALE` 与前端一致。
- 自动验证通过：`npm run typecheck`、`npm test`（55 项）、`npm run build`、`npm run test:e2e`（2 项）、`cargo fmt --check`、`cargo clippy -- -D warnings`、`cargo test`（33 项通过，2 项手动延迟测试默认忽略）。
- 勾选 2 的依据：#07 已于 `98a9ea4` 合并，`PageTextLayer` 以页面宽高百分比定位，`PageTextLayer.test.tsx` 断言高亮百分比几何不随容器像素尺寸变化，`App.test.tsx` 验证缩放只重渲染底图而不重新提取文本，Rust `text_layer.rs` 校验文本框与渲染墨迹、旋转页与 CropBox 对齐。4000% 分块渲染下与文本层的像素级对齐仍建议桌面端冒烟。
- 勾选 1/3/4/5 的依据：App 测试覆盖页码输入、上一页/下一页、Home/End，缩略图懒渲染/缓存/点击跳转与活动缩略图滚动，全屏进退恢复页码与缩放，旋转页与混合页面尺寸的 surface 宽高比；`navigation.rs` 覆盖 `page_info` 的旋转/显示尺寸、缩略图长边上限、高倍率 region 与整页裁剪的一致性（含旋转页）以及长文档缩略图/区域延迟。
- 原生 GUI 未做人工 live 测试：真实窗口的原生全屏切换、缩略图滚动的实际流畅度、4000% 下 region 平移的手感仅由 mock Tauri 边界与 Rust 命令测试覆盖，需桌面端手工冒烟。
- 既有问题（与本 ticket 无关，合并前即存在）：`cargo clippy --all-targets -- -D warnings` 因 `src-tauri/tests/support/fixtures.rs` 的 dead_code 失败（另有 `tests/fixtures.rs` 的 `Fixture` 字段）；按约定不在本 ticket 修复。合并前的 `cargo fmt --check` 差异（`menu.rs`、`tests/fixtures.rs`）已由 `e920803` 与后续 #06/#07 分支的同一修复覆盖。
