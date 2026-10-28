# PDFX

本地运行、跨 macOS 与 Windows 的 PDF 编辑器。界面参照 iLovePDF 编辑页的信息架构与交互密度，但文件始终留在本机，无账户、无云端处理、无遥测。

当前仓库已包含 **票 01：仓库脚手架**、**票 02：M0 浏览器原型**、**票 03：PDF 引擎技术验证**（结论见 [`docs/spikes/pdf-engine.md`](docs/spikes/pdf-engine.md)）与 **票 04：打开文档并渲染第一页**。生产桌面壳可通过文件对话框或拖放打开本地 PDF，在独立窗口中用 PDFium 渲染第 1 页；编辑、保存与翻页尚未实现。

## 技术栈

- **桌面壳**：Tauri 2（ADR-0001），原生标题栏与系统菜单
- **前端**：React 19 + TypeScript + Vite
- **后端**：Rust，负责文件系统、PDF 处理与命令编排
- **测试**：Rust 集成测试（命令层）+ Vitest / Testing Library（mock Tauri IPC 边界与原型交互）+ Playwright（原型布局冒烟）
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

完整安装包（`.dmg` / `.msi` / `.exe` / portable zip）属于分发票的范围；本仓库暂不包含签名、公证或自动更新。

## 命令

| 命令 | 说明 |
| --- | --- |
| `npm run typecheck` | TypeScript 严格模式类型检查 |
| `npm test` | Vitest 单元/组件测试（mock Tauri IPC、事件、窗口、拖放与文件对话框边界；含原型交互与中文文案守卫） |
| `npm run test:e2e` | Playwright 原型布局冒烟（1440×900 与 1280×720），首次需 `npx playwright install chromium` |
| `npm run build` | 类型检查 + Vite 生产构建 |
| `npm run setup:native` | 下载并校验固定版本 PDFium 原生库到 `src-tauri/vendor/`（不提交） |
| `npm run prototype` | 启动开发服务器并在浏览器打开 `/prototype.html` |
| `cd src-tauri && cargo check` | Rust 编译检查 |
| `cd src-tauri && cargo test` | 命令层、打开/渲染与 fixture 加载器测试（需先 `npm run setup:native`） |
| `python3 fixtures/generate.py --check` | 校验已提交语料与生成器一致 |
| `swift fixtures/verify.swift` | 用 macOS PDFKit 独立验证每个 fixture |

## 目录结构

```
prototype.html        M0 原型入口（浏览器，不含 Tauri IPC）
e2e/                  Playwright 原型布局冒烟
scripts/              原生依赖准备脚本（setup-native.sh）
src/                  React 前端
  lib/backend.ts      Tauri 命令绑定的唯一前端入口
  App.tsx             生产工作区：打开文档、渲染第 1 页、错误与缩放
  prototype/          M0 原型（PrototypeApp、文案、占位文档、样式）
src-tauri/            Tauri / Rust
  src/commands.rs     Tauri 命令边界（app_info、open_document、render_page 等）
  src/engine.rs       PDFium 动态库绑定与字节加载
  src/menu.rs         原生菜单（文件/编辑/视图/帮助）
  tests/              命令层、打开/渲染与 fixture 加载器集成测试
  vendor/             本地 PDFium 原生库（git-ignored）
fixtures/             固定 PDF 语料与生成/验证脚本
docs/adr/             架构决策记录
playwright.config.ts  原型 e2e 配置（两个桌面视口）
```

## 测试 seam

唯一主 seam 是 **Tauri 命令边界**：

- Rust 侧命令函数（`app_info`、`open_document`、`render_page`、`extract_text`、`close_document`）由 `cargo test` 直接调用，断言跨 IPC 序列化后的结果；打开/渲染测试覆盖文本、扫描、加密、截断、坏 xref、旋转与 500 页语料。
- 前端只通过 `src/lib/backend.ts` 调用 `invoke`；组件测试 mock Tauri 系统边界（`@tauri-apps/api/core`、`event`、`window`、`webview` 与 `@tauri-apps/plugin-dialog`），不 mock 自己的模块。
- 测试不接触 PDFium 内部类型；引擎测试同样通过命令边界驱动。

新增命令时：在 `src-tauri/src/commands.rs` 注册，在 `src/lib/backend.ts` 增加对应调用，测试分别驱动命令函数与 mock 后的 IPC。

M0 原型还有两个独立 seam：

- **渲染后的界面**：`src/prototype/PrototypeApp.test.tsx` 通过可访问角色驱动模式切换、页签、工具、翻页、缩放与属性选择；不触碰组件内部状态。
- **真实浏览器布局**：`e2e/prototype-layout.spec.ts` 在 1440×900 与 1280×720 下断言无溢出、各区域位于视口内且控件同步。也可手动在这两个视口打开 `/prototype.html` 复核。

## Fixture 语料

`fixtures/` 覆盖文本、扫描、加密（用户密码与所有者权限限制）、表单、结构（书签/链接/元数据/附件）、500 页大文件、旋转/混合尺寸、损坏文件。密码与生成方式见 [`fixtures/README.md`](fixtures/README.md)。

语料由 `fixtures/generate.py` 以标准库确定性生成；`fixtures/verify.swift` 用 PDFKit（与生成器不同的实现）验证加密、文本层、旋转、表单 Widget、书签/链接与截断文件行为，避免自证正确。

## 依赖许可

仅使用宽松许可依赖：Tauri、React、Vite、Vitest、Testing Library、`pdfium-render` 为 MIT / Apache-2.0，TypeScript 为 Apache-2.0。PDFium 原生库为 BSD-3-Clause（含第三方许可，见解压后的 `src-tauri/vendor/pdfium/LICENSE`），由 setup 脚本按固定 SHA-256 获取，不随仓库分发。仓库不含付费服务、遥测或代码签名配置。

## 当前限制

- 只渲染第 1 页；翻页、连续滚动与页面导航属于后续票据。
- 编辑与保存尚未实现；脏状态与关闭保护只有脚手架，当前始终按“未修改”处理。
- 加密 PDF 在命令层返回 `password_required` / `invalid_password`，界面尚未提供密码输入框。
- 每个文档窗口独立打开一个文档；尚无窗口间会话恢复或最近文件列表。
- 原生 PDFium 库需要本地运行 `npm run setup:native`；仓库不提交二进制。
- M0 原型（票 02）只验证布局、交互与文案；不打开真实 PDF，`插入`、`表单` 等后续能力显示为占位状态。
- fixture 中的“大文件”是 500 页的提交版本；可用 `--large-pages 2000` 生成完整压力样例。
- 中文/Embedded Font/Font Subset fixture 待字体管线票补充（见 `fixtures/README.md`）。
- 生产构建目前未打包、未签名；GUI 启动冒烟需在图形会话中执行。
