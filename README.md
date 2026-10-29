# PDFX

本地运行、跨 macOS 与 Windows 的 PDF 编辑器。界面参照 iLovePDF 编辑页的信息架构与交互密度，但文件始终留在本机，无账户、无云端处理、无遥测。

当前集成分支已关闭票 01–47。生产桌面壳可通过文件对话框、拖放或最近文件打开本地 PDF，支持逐页渲染与缩略图、缩放与全屏、覆盖式文字编辑与文本样式、文本注释与手绘/便签、形状与线条、图片与签名、链接、图章、表单创建与填写、页面旋转/删除/排序/插入空白页/复制、水印、附件、图层、打印与 PNG/JPG 导出、Flatten 副本导出、加密文档打开与加密输出、崩溃恢复、设置与本地日志、深色模式、未保存标记与关闭确认、用户发起的更新检查，以及 Save As / Save Changes。初始版本未内置全部 M0 原型的信息架构与部分规格能力，实际边界见 [`docs/known-limitations.md`](docs/known-limitations.md)。

## 技术栈

- **桌面壳**：Tauri 2（ADR-0001），原生标题栏与系统菜单
- **前端**：React 19 + TypeScript + Vite
- **后端**：Rust，负责文件系统、PDF 处理与命令编排
- **PDF 引擎**：PDFium + lopdf 混合架构（ADR-0010）；PDFium 负责渲染、文本提取与支持的 Native Annotation，lopdf 负责对象级编辑与增量保存
- **测试**：Rust 集成测试（命令层与适配层）+ Vitest / Testing Library（mock Tauri 系统边界与原型交互）+ Playwright（原型布局冒烟）
- **语料**：`fixtures/`，纯 Python 生成，macOS PDFKit 独立验证

目标平台：macOS 12+、Windows 10 1809+ / 11 x64。

## 环境要求

- Node.js `^22.13.0` 或 `>=24`（Vite 8 / jsdom 29 要求）；npm
- Rust `>=1.90`（Tauri 2.12 的 MSRV）
- macOS 构建需要 Xcode Command Line Tools；Windows 构建需要 MSVC 工具链与 WebView2

## 快速开始

```bash
npm install
npm run setup:native   # 一次性：下载、校验并解压 PDFium 原生库
npm run tauri dev      # 启动 Vite 与 Tauri 开发窗口
npm run prototype      # 在浏览器中打开 M0 原型（/prototype.html）
```

`npm run setup:native` 从 bblanchon/pdfium-binaries 的 `chromium/7881` 发布页下载与 `pdfium-render =0.9.4` 匹配的 PDFium 151.0.7881.0（macOS universal、Linux x64、Windows x64），校验 SHA-256 后解压到 `src-tauri/vendor/pdfium/`。该目录已被 git 忽略，仓库不提交原生二进制。若把库放在别处，设置 `PDFIUM_DYNAMIC_LIB_PATH` 指向库文件。缺少原生库时应用仍可启动，但打开文档会返回带安装指引的结构化错误；`cargo test` 也要求先执行该脚本。

M0 原型只运行在浏览器中：入口为 `prototype.html`，不加载 Tauri IPC，也不调用任何 PDF 引擎。生产壳入口 `src/App.tsx` 已接入 PDFium，但在普通浏览器中只显示空工作区。

生产构建（不打包安装器，便于本机冒烟）：

```bash
npx tauri build --no-bundle
./src-tauri/target/release/pdfx
```

## macOS 打包与安装

macOS 提供两个未签名、未公证的构建，最低 macOS 12：

- `npm run build:macos:arm64`：Apple Silicon 专用。脚本会把应用包内的 PDFium 也裁剪为 arm64，并检查包内每个 Mach-O 文件都不含 x86_64 切片。macOS 26.4+ 不再把 PDFX 报告为依赖 Intel 组件的应用。
- `npm run build:macos`：Apple Silicon + Intel 的 universal 构建。Intel Mac 必须使用这一版；由于包内保留 x86_64 切片，macOS 26.4+ 在 Apple Silicon 上仍可能显示“对 Intel 芯片 App 的支持即将结束”的系统通知。这是 Apple 的弃用提示，不是 PDFX 依赖 Rosetta。

```bash
npm ci
npm run build:macos:arm64   # Apple Silicon
# 或
npm run build:macos         # universal，含 Intel
```

`scripts/build-macos-arm64.sh` 会先运行原生依赖脚本，检查 `aarch64-apple-darwin` Rust 目标与 PDFium 的 arm64 切片，随后执行 `tauri build --target aarch64-apple-darwin --bundles app`，把 `Contents/Frameworks/libpdfium.dylib` 裁剪为 arm64，以 ad-hoc 方式重新签名，最后自行生成 `.dmg` 并用 `PDFX_EXPECTED_ARCH=arm64` 运行同一个校验脚本。`scripts/build-macos.sh` 则检查 `aarch64-apple-darwin` 与 `x86_64-apple-darwin` 两个 Rust 目标和 universal PDFium，执行 `tauri build --target universal-apple-darwin --bundles app,dmg`，最后用 `scripts/verify-macos-bundle.sh` 检查可执行文件与 PDFium 的架构、`Contents/Frameworks` 布局、Info.plist 文件关联、许可证与 `.dmg` 内容。两种构建都通过 `src-tauri/Info.plist` 把 Tauri 默认写入的 `LSRequiresCarbon=true` 覆盖为 `false`（PDFX 是 Cocoa 应用，不需要旧式 Carbon 环境），并在 `src-tauri/tauri.macos.conf.json` 中把 `signingIdentity` 设为 `-`，让 Tauri 在打包阶段完成 ad-hoc 签名（不是 Developer ID 签名，也不公证）。只校验已有产物可运行 `npm run verify:macos` 或 `npm run verify:macos:arm64`。

产物位置：

- `src-tauri/target/aarch64-apple-darwin/release/bundle/macos/PDFX.app`
- `src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/PDFX_0.1.0_arm64.dmg`
- `src-tauri/target/universal-apple-darwin/release/bundle/macos/PDFX.app`
- `src-tauri/target/universal-apple-darwin/release/bundle/dmg/PDFX_0.1.0_universal.dmg`

首次启动会被 Gatekeeper 拦截。右键/Control 点击“打开”、在“系统设置 → 隐私与安全性”点击“仍要打开”，或执行 `xattr -dr com.apple.quarantine /Applications/PDFX.app` 即可放行。完整说明见 [`docs/install-macos.md`](docs/install-macos.md)，同一文件会随应用放入 `Contents/Resources/INSTALL-macos.md`。不购买 Apple Developer Program、不做公证是 ADR-0009 的零成本分发取舍：用户需要手动放行，应用也不会自动更新。Windows 安装器（`.msi` / `.exe` / portable zip）由票 18 提供，见 [`docs/install-windows.md`](docs/install-windows.md)。

## 命令

| 命令 | 说明 |
| --- | --- |
| `npm run typecheck` | TypeScript 严格模式类型检查 |
| `npm test` | Vitest 单元/组件测试（mock Tauri 系统 seam、事件、窗口、拖放与文件对话框；含原型交互与中文文案守卫） |
| `npm run test:e2e` | Playwright 原型布局与主题冒烟（1440×900 与 1280×720），首次需 `npx playwright install chromium` |
| `npm run build` | 类型检查 + Vite 生产构建 |
| `npm run setup:native` | 下载并校验固定版本 PDFium 原生库到 `src-tauri/vendor/`（不提交） |
| `npm run build:macos` | 构建未签名的 universal macOS `.app` 与 `.dmg`，并校验产物 |
| `npm run build:macos:arm64` | 构建 Apple Silicon 专用、无 x86_64 切片的 `.app` 与 `.dmg`，并校验产物 |
| `npm run verify:macos` | 校验已构建的 universal `.app` / `.dmg`（架构、框架、Info.plist、许可证） |
| `npm run verify:macos:arm64` | 校验 Apple Silicon 专用产物，并拒绝任何 x86_64 切片 |
| `npm run prototype` | 启动开发服务器并在浏览器打开 `/prototype.html` |
| `cd src-tauri && cargo check` | Rust 编译检查 |
| `cd src-tauri && cargo test` | 命令层、打开/渲染与 fixture 加载器测试（需先 `npm run setup:native`） |
| `python3 fixtures/generate.py --check` | 校验已提交语料与生成器一致 |
| `swift fixtures/verify.swift` | 用 macOS PDFKit 独立验证每个 fixture |

## 目录结构

```
prototype.html        M0 原型入口（浏览器，不含 Tauri IPC）
e2e/                  Playwright 原型布局与主题冒烟
scripts/              原生依赖准备与 macOS 打包/校验脚本（setup-native.sh、build-macos.sh、verify-macos-bundle.sh）
src/                  React 前端
  lib/backend.ts      Tauri 命令、事件、窗口与文件对话框的唯一前端入口
  lib/                设置、恢复、缩放/几何与文案等前端逻辑
  components/         页面覆盖层、面板与对话框（文字、注释、形状、图片、链接、表单、图层等）
  App.tsx             生产工作区：文档会话、菜单、保存、关闭守卫与各面板编排
  prototype/          M0 原型（PrototypeApp、文案、占位文档、样式）
src-tauri/            Tauri / Rust
  src/lib.rs          Tauri 构建入口与 `generate_handler!` 命令注册
  src/commands.rs     跨领域命令、文档注册表与共享命令边界类型（打开/渲染/文本提取/设置等）
  src/*.rs            功能模块：各自拥有对应的 `#[tauri::command]` 函数（annotations、links、save、page_ops、form 等）
  src/engine.rs       PDFium 动态库绑定与字节加载
  src/menu.rs         原生菜单（文件/编辑/视图/帮助）
  tests/              命令边界、PDFium 适配层与 fixture 加载器集成测试
  vendor/             本地 PDFium 原生库（git-ignored）与 vendored `pdfium-render`
fixtures/             固定 PDF 语料与生成/验证脚本
docs/adr/             架构决策记录
docs/known-limitations.md  与规格/原型的已知差异及未验证项
playwright.config.ts  原型 e2e 配置（两个桌面视口）
```

## 测试 seam

唯一主 seam 是 **Tauri 命令边界**：

- Rust 侧：多数集成测试直接调用命令函数（`app_info`、`open_document`、`render_page`、`extract_text`、`save_document`、`create_link` 等），断言跨 IPC 序列化后的结果；覆盖文本、扫描、加密、截断、坏 xref、旋转、页面操作、注释、链接、表单、保存与 500 页语料。`src-tauri/tests/pdfium_adapter.rs` 是例外：它刻意使用 PDFium 类型，验证 vendored 适配层与 PDFium 的行为契约，不经过命令边界。
- 前端：产品代码只通过 `src/lib/backend.ts` 调用 `invoke` 或打开原生对话框。组件测试 mock `src/lib/backend.ts`（Tauri 系统 seam）以隔离 IPC；部分既有测试也直接 mock `@tauri-apps/api/core`、`event`、`window`、`webview` 与 `@tauri-apps/plugin-dialog`，两者都属于系统边界。组件测试不 mock 兄弟组件。
- 测试可以接触 PDFium 类型仅限适配层契约测试；其余引擎行为通过命令边界驱动。

新增命令时：在拥有该功能的 Rust 模块中实现 `#[tauri::command]` 函数，在 `src-tauri/src/lib.rs` 的 `generate_handler!` 中注册，并在 `src/lib/backend.ts` 增加对应包装与测试。跨领域的文档会话、打开/渲染与设置命令放在 `src-tauri/src/commands.rs`。

M0 原型还有两个独立 seam：

- **渲染后的界面**：`src/prototype/PrototypeApp.test.tsx` 通过可访问角色驱动模式切换、页签、工具、翻页、缩放与属性选择；不触碰组件内部状态。
- **真实浏览器布局**：`e2e/prototype-layout.spec.ts` 与 `e2e/theme.spec.ts` 在 1440×900 与 1280×720 下断言无溢出、各区域位于视口内且控件/主题同步。也可手动在这两个视口打开 `/prototype.html` 复核。

## Fixture 语料

`fixtures/` 覆盖文本、扫描、加密（用户密码与所有者权限限制）、表单、结构（书签/链接/元数据/附件）、500 页大文件、旋转/混合尺寸、损坏文件。密码与生成方式见 [`fixtures/README.md`](fixtures/README.md)。

语料由 `fixtures/generate.py` 以标准库确定性生成；`fixtures/verify.swift` 用 PDFKit（与生成器不同的实现）验证加密、文本层、旋转、表单 Widget、书签/链接与截断文件行为，避免自证正确。

## 依赖许可

仅使用宽松许可依赖：Tauri、React、Vite、Vitest、Testing Library、`pdfium-render` 为 MIT / Apache-2.0，TypeScript 为 Apache-2.0。PDFium 原生库为 BSD-3-Clause（含第三方许可，见解压后的 `src-tauri/vendor/pdfium/LICENSE`），由 setup 脚本按固定 SHA-256 获取，不随仓库分发；macOS 包会把 `LICENSE` 与完整 `licenses/` 目录放入 `Contents/Resources/pdfium-licenses/`。仓库不含付费服务、遥测或代码签名配置。

## 当前限制

与规格、M0 原型和原定验收范围相比的完整差异，以及未执行的人工验证，见 [`docs/known-limitations.md`](docs/known-limitations.md)。其中重要的有：生产工作区没有实现原型顶部的“注释/编辑”模式切换与五个页签；只按需渲染当前页；界面语言设置只更新 `<html lang>`；初始版本只内置 Noto Sans SC 与 PDF base-14 字体；应用级退出不经过未保存确认；打印只限制解码位图而不限制压缩 PNG Blob；等。

- 原生 PDFium 库需要本地运行 `npm run setup:native`；仓库不提交二进制。
- M0 原型（票 02）只验证布局、交互与文案；不打开真实 PDF，后续能力显示为占位状态。
- fixture 中的“大文件”是 500 页的提交版本；可用 `--large-pages 2000` 生成完整压力样例。
- 生产构建目前未签名、未公证；macOS 首次启动需按 [`docs/install-macos.md`](docs/install-macos.md) 绕过 Gatekeeper。Intel 硬件与 macOS 12 上的实机启动仍需在发布前用对应机器验证；仓库内的自动化检查只能验证 universal / arm64 架构与包结构。
- Windows 安装器（`.msi` / `.exe` / portable zip）见 [`docs/install-windows.md`](docs/install-windows.md)；尚未在 Windows 实机验证。
