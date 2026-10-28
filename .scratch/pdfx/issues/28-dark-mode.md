# 28: 深色模式

**What to build:** PDFX 在 M2 跟随系统深色模式，工作区、面板、菜单与画布周边一致切换；用户可在设置里固定浅色或深色。

**Blocked by:** 16 (设置、持久化与日志)

**Status:** done

- [x] 系统切换深色/浅色时应用即时跟随（设置未固定时）。
- [x] 深色模式下文本、图标、边框、选中态对比度符合可读性。
- [x] 画布 PDF 内容不被反色；只有 UI 主题变化。
- [x] 设置里的主题选择在重启后保留。

**Verification notes:**

- 合并提交 `efe63ba`（ticket/28-dark-mode `58f7619`，feature `41fe2bf`，merge base `3548c61`），`git merge --no-ff`。ticket 分支此前已把 `3548c61` 合入并解决唯一冲突 `src-tauri/src/lib.rs`（见 `58f7619`）：采用集成分支的命令顺序（`get_settings`/`update_settings`/`export_diagnostics` 位于 text_edit 命令之后），两侧命令注册均保留；合并后该文件与集成 tip 完全一致，本次合并无冲突。ticket 未修改 `src-tauri/`、`package.json`、`package-lock.json`、`Cargo.toml`/`Cargo.lock`、spec、ADR 或打包配置。
- 勾选 1：`src/theme.css` 在 `:root[data-theme="dark"]` 与 `@media (prefers-color-scheme: dark)` 的 `:root:not([data-theme])` 中定义同一套深色 token；`applyTheme("system")` 移除 `data-theme`，由媒体查询即时重绘，无需重载。`useSettings` 在主题为 system 时订阅 `matchMedia` 变化并同步 `data-resolved-theme`；`theme.ts` 单元测试覆盖 `resolveTheme`/`watchSystemTheme`/缓存/启动应用，`useSettings.test.tsx` 覆盖“system 即时跟随”“固定主题忽略 OS 变化”“设置不可用时保留缓存主题”。Playwright `e2e/theme.spec.ts` 在真实浏览器中 `emulateMedia` light→dark→light，断言根背景与 `.app-header` 计算色即时变化，且固定深/浅主题在相反 OS 下不被覆盖。
- 勾选 2：`src/theme.test.ts` 直接解析真实 CSS，要求浅色、固定深色、系统深色三套 palette 都定义 61 个 token 且名称同步；逐 theme 断言 22 组正文前景/背景 ≥ 4.5:1、6 组控件/焦点/选中态 ≥ 3:1，PDF 页面上的文字与控件另行断言（3 组文字 ≥ 4.5:1、2 组控件 ≥ 3:1），全部通过。`styles.css`/`settings.css` 中已无硬编码 hex/rgba（测试强制），生产 TSX 无内联颜色样式或硬编码 SVG 填充色（生产界面无图标组件，按钮为文字）。完整界面的肉眼视觉检查仍需打包应用人工执行。
- 勾选 3：三套 palette 中 `--pdfx-page #ffffff`、`--pdfx-page-ink #1f2430`、`--pdfx-page-control-bg #ffffff` 完全一致；`theme.css`/`styles.css`/`settings.css` 均无 `filter` 或 `mix-blend-mode`，PDF 渲染与保存链路（`src-tauri/`）未改动。Playwright 在固定深色下注入 `.page-surface`/`.page-image`，断言计算背景 `rgb(255,255,255)`、`filter: none`、`mix-blend-mode: normal` 且 `--pdfx-page` 为 `#ffffff`；页面上的文本框编辑浮层使用 page token（白底深字），不随 UI 反色。
- 勾选 4：`Settings.theme` 由 ticket 16 的后端 store 持久化到 `settings.json`，`settings_survive_a_restart_through_a_new_store` 以 `Theme::Dark` 保存后重建 store 断言完全相等；前端设置页改动经 `update_settings` 保存并应用返回值，启动时读取后端并 `applySettings`（既有 `useSettings` 测试断言加载后写入 `data-theme`）。另外 `initializeTheme()` 在首个 React render 前读取 localStorage 镜像，固定主题不会先闪浅色；后端仍是唯一真源，缓存不可用时静默回退。
- 隐私与依赖：diff 中无 `fetch`/`XMLHttpRequest`/`WebSocket`/发送上报调用，设置页无遥测开关；`package.json`/`package-lock.json`/`Cargo.toml`/`Cargo.lock` 相对 `3548c61` 无变化，未新增依赖或许可证。
- 中文文案：设置面板主题选项为“跟随系统 / 浅色 / 深色”，其余设置文案保持不变；`index.html` 的 `color-scheme` 改为 `light dark`，原生表单控件随主题切换。
- 自动化验证（合并后于仓库根执行）：`npm run typecheck`、`npm test`（18 个文件、171 项，含本 ticket 新增 36 项）、`npm run build`、`npm run test:e2e`（12 项：主题 5 项 × 2 视口 + 原型布局 2 项）全部通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 的 4 条 dead_code warning）；`cargo test` 104 项通过、3 项手动/延迟测试忽略（与合并前基线一致，本 ticket 未改 Rust）。`npm ci` 与 `npm run setup:native` 未重跑：`node_modules` 与 `src-tauri/vendor/pdfium` 均存在且 lockfile/脚本未变。
- 未验证的人工 GUI 项：真实 Tauri 窗口（macOS WKWebView / Windows WebView2）中的系统外观即时切换、固定主题下的启动无闪烁、全部面板/对话框/缩略图/选中态的视觉检查。已知限制：原生标题栏与系统菜单由 OS 控制，固定应用主题不会重绘它们（跟随系统时与 UI 一致）；如需让窗口 chrome 跟随固定主题，后续可在 Rust 侧调用 Tauri window theme API，但 macOS 菜单栏本身无法由应用改色。
- 后续/交叉 ticket：`src/theme.test.ts` 只扫描 `styles.css` 与 `settings.css`，后续新增生产 CSS 文件需加入该测试才能被 token/无硬编码颜色约束覆盖；`src/prototype/prototype.css` 仍是 M0 独立原型的浅色实现，不随生产主题切换。固定深色与系统深色的 token 名称被测试强制同步，但两套值之间没有逐项相等断言（当前人工核对一致），可择机加强。
