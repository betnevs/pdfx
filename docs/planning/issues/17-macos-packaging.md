# 17: macOS 打包

**What to build:** 产出 macOS 12+ 的 universal 构建与 `.dmg`，包含未签名安装说明；Apple Silicon 与 Intel 都能启动。

**Blocked by:** 04 (打开文档并渲染第一页)

**Status:** done

- [ ] universal `.dmg` 在 Apple Silicon 与 Intel macOS 12+ 上可安装并启动。
- [x] 首次启动的 Gatekeeper 绕过说明（系统设置“仍要打开”或 `xattr` 命令）随包提供。
- [x] 应用名称、图标、版本号与 PDFX 品牌一致；文件关联与 Info.plist 配置正确。
- [x] 不包含 Apple 公证或代码签名；文档明确说明这是零成本分发的取舍。

**Verification notes:**

- 合并提交 `2850770`（ticket/17-macos-packaging `0b106ae`，feature `b7699a4`）。`git merge --no-ff` 无冲突：ticket 18 合入后未再修改 README/package.json/tauri.conf.json，预期冲突未出现。未修改 `src-tauri/tauri.windows.conf.json`、spec、ADR 或其他 ticket。合并后另提交 `0d9c81e`，仅修正 README 中“Windows 安装器尚未提供”的过期表述（ticket 17 分支在 ticket 18 合入前编写，合并时已过时）。
- 配置审查：`src-tauri/tauri.macos.conf.json` 定义 `fileAssociations`（`pdf`、`application/pdf`、Editor、Default）、`macOS.frameworks=["vendor/pdfium/lib/libpdfium.dylib"]`、`minimumSystemVersion="12.0"`、`hardenedRuntime=false` 与 `resources`（`../docs/install-macos.md` → `INSTALL-macos.md`，`vendor/pdfium/LICENSE`、`vendor/pdfium/licenses/` → `pdfium-licenses/`）；基础 `tauri.conf.json` 删除重复的 `macOS.minimumSystemVersion`。配置中无签名、公证、自动更新或遥测项。
- 自动验证通过：`npm ci`、`npm run setup:native`、`npm run typecheck`、`npm test`（8 个文件、88 项）、`npm run build`、`npm run test:e2e`（2 项）、`cargo fmt --check`、`cargo clippy -- -D warnings`（exit 0；仅 vendored `pdfium-render` 的 4 条 dead_code warning）、`cargo test`（50 项通过，2 项手动延迟测试默认忽略）。
- `npm run build:macos` exit 0：universal release 构建与打包完成，产出 `PDFX.app`（37.11 MiB）与 `PDFX_0.1.0_universal.dmg`（13.78 MiB，SHA-256 `d31f2ac1a496c0cf65c2107ecf2ca6d8f149ac329779b836583d02a562d07471`）；脚本内置的校验全部通过。随后独立重跑 `npm run verify:macos` exit 0，13 项检查全部 `ok`。
- 产物证据：可执行文件与 `Contents/Frameworks/libpdfium.dylib` 均为 `x86_64 arm64` universal；`CFBundleName=PDFX`、`CFBundleIdentifier=app.pdfx.desktop`、`CFBundleShortVersionString=CFBundleVersion=0.1.0`、`LSMinimumSystemVersion=12.0`、`CFBundleIconFile=icon.icns`；`CFBundleDocumentTypes=[{pdf, com.adobe.pdf, Editor, Default}]`；存在 `@executable_path/../Frameworks` rpath；仅有 ad-hoc/linker-signed 签名（无 Developer ID Authority），`stapler validate` 报告无公证 ticket；DMG checksum、挂载、`PDFX.app`/framework/`Applications` 快捷方式均通过；`pdfium-licenses/` 含 16 个许可证文件，`INSTALL-macos.md` 含 `xattr -dr com.apple.quarantine` 与“仍要打开”。
- 启动冒烟：运行 `PDFX.app/Contents/MacOS/pdfx`，输出 `[pdfx] workspace ready` 且无 `[pdfx] PDF engine unavailable`（说明包内 PDFium 已加载），随后终止；再用 `arch -x86_64` 走 Rosetta 重复，结果相同。两次均确认无残留进程。宿主为 macOS 26.6.2 arm64，不是 macOS 12 或 Intel 硬件。
- 勾选 2：绕过说明存在于 `docs/install-macos.md`、README 与包内 `Contents/Resources/INSTALL-macos.md`，覆盖系统设置“仍要打开”与 `xattr` 两条路径，校验脚本断言通过。
- 勾选 3：应用名、图标、版本与 Info.plist 文件关联均在真实产物上断言通过；Finder/LaunchServices 的实际默认打开行为未做 live 测试。
- 勾选 4：产物无 Developer ID 签名、无公证 ticket；`docs/install-macos.md`、README 与 ADR-0009 说明零成本分发取舍。Apple Silicon 所需的 ad-hoc 链接器签名不属于 Apple Developer 代码签名。
- 未勾选 1：无真实 Intel 硬件与 macOS 12 机器。只验证了 universal 双架构、DMG 结构与 x86_64 Rosetta 启动；真实 Intel/macOS 12 上的安装与启动、以及实机 Gatekeeper 拦截 UI（本机构建无 quarantine 属性，不会触发）仍需发布前验证。
- 既有问题（与本 ticket 无关）：`cargo fmt --check` 现已通过；`cargo clippy --all-targets -- -D warnings` 仍因 `src-tauri/tests/support/fixtures.rs` 与 `tests/support/synthetic.rs` 的 dead_code 失败（17 条 never used），按约定 gate 使用不带 `--all-targets` 的 clippy。
