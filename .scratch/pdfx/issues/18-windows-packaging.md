# 18: Windows 打包

**What to build:** 产出 Windows 10 1809+/11 x64 的 `.msi`、`.exe` 安装器与 portable zip，内置 WebView2 Evergreen Bootstrapper；提供 SmartScreen 绕过说明。

**Blocked by:** 04 (打开文档并渲染第一页)

**Status:** done

- [ ] `.msi`、`.exe` 与 portable zip 在 Windows 10 1809+/11 x64 上可安装/启动。
- [ ] 离线环境下 WebView2 Bootstrapper 能完成安装；不依赖联网的 0MB bootstrapper。
- [x] SmartScreen 的“更多信息 → 仍要运行”说明随包提供。
- [x] 应用名称、图标、版本号与 PDFX 品牌一致；文件关联配置正确。
- [x] 不包含 Windows 代码签名；文档明确说明这是零成本分发的取舍。

**Verification notes:**

- 合并提交 `19e0ffd`（ticket/18-windows-packaging `7a557ff`）。仅新增 4 个文件（`src-tauri/tauri.windows.conf.json`、`scripts/build-windows.ps1`、`scripts/verify-windows-config.mjs`、`docs/install-windows.md`），未修改共享配置、spec 或 ADR。
- 自动验证通过：`npm ci`、`npm run setup:native`、`npm run typecheck`、`npm test`（29 项）、`npm run build`、`npm run test:e2e`（2 项）、`cargo test`（19 项通过，1 项默认忽略）、`node scripts/verify-windows-config.mjs`（42/42 项）。`npx tauri inspect wix-upgrade-code` 输出 `5b42853e-7b88-59d0-b9c4-7af5016a1dfd`，与 Windows 配置中的 `wix.upgradeCode` 一致。
- 未勾选 1：macOS 上无法产出或启动 Windows 安装包。需要在干净的 Windows 10 1809 与 11 x64 虚拟机上实测 MSI/NSIS 安装与卸载、启动、打开 PDF，以及 portable zip 解压启动。静态检查已覆盖目标三元组 `x86_64-pc-windows-msvc`、`--bundles msi,nsis`、产物命名与 PE32+ x64 校验。
- 未勾选 2：合并后的有效 WebView2 模式为 `offlineInstaller`（内嵌 Evergreen Standalone Installer，`silent: true`），覆盖了基础配置中的 `embedBootstrapper`，满足“不依赖联网 bootstrapper”的配置要求；但离线安装过程未在 Windows 上实际执行。体积取舍：`offlineInstaller` 使安装包约增大 127 MB，而基础配置的 `embedBootstrapper` 约 +1.8 MB。spec §“分发与更新”/“运行时与平台”仍写“约 +1.8MB”，与离线验收标准存在冲突，需在 spec 跟进（本 ticket 未修改 spec/ADR）。
- 勾选 3：`docs/install-windows.md` 含“更多信息 → 仍要运行”与 `Unblock-File` 说明；Windows 配置将该文档映射为安装包内的 `INSTALL-windows.md`，`build-windows.ps1` 也将其复制进 portable zip，静态校验通过。安装包内的实际文件未在 Windows 上打开核对。
- 勾选 4：`productName` 为 `PDFX`，图标列表包含 `icons/icon.ico` 且文件存在，版本 `0.1.0` 在 `tauri.conf.json`、`Cargo.toml`、`package.json` 中一致；`tauri.windows.conf.json` 为 `.pdf` 注册 `application/pdf` / `Editor` 文件关联，并被 Windows target 的配置解析接受。实际的系统注册行为需在 Windows 上确认。
- 勾选 5：配置与脚本不含代码签名设置，`build-windows.ps1` 显式使用 `--no-sign` 且无 `signtool`；文档说明未签名并链接 ADR-0009。
- Windows target 编译：未暂存 Windows `pdfium.dll` 时，`cargo check --target x86_64-pc-windows-msvc` 停在 tauri-build 的资源存在性检查（`vendor/pdfium/bin/pdfium.dll` doesn't exist）；`setup-native.sh` 只按宿主平台下载，无法非破坏性提供 Windows DLL。改用 `TAURI_CONFIG='{"bundle":{"resources":null}}'` 暂时跳过该宿主检查后，Rust 代码的 Windows 交叉编译检查通过（exit 0，仅 `src/menu.rs` 既有 warning）；真实 Windows 构建由 `build-windows.ps1` 先运行 `setup:native` 提供 DLL。
- 既有问题（与本 ticket 无关，合并前即存在）：`cargo fmt --check` 对 `src-tauri/src/menu.rs` 与 `src-tauri/tests/fixtures.rs` 报格式差异；`cargo clippy --all-targets -- -D warnings` 因 `src-tauri/tests/support/fixtures.rs` 的 dead_code 失败。两处文件均自 `9e3f338` 起未改动，建议另开维护 ticket 修复。
