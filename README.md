# PDFX

本地运行、跨 macOS 与 Windows 的 PDF 编辑器。界面参照 iLovePDF 编辑页的信息架构与交互密度，但文件始终留在本机，无账户、无云端处理、无遥测。

当前仓库处于 **票 01：仓库脚手架** 阶段：可启动的 Tauri 2 桌面壳、React + TypeScript 空工作区、原生标题栏与菜单、唯一的 Tauri 命令 seam，以及固定 fixture 语料。完整编辑器界面由后续票实现；PDF 引擎（PDFium/lopdf）尚未接入。

## 技术栈

- **桌面壳**：Tauri 2（ADR-0001），原生标题栏与系统菜单
- **前端**：React 19 + TypeScript + Vite
- **后端**：Rust，负责文件系统、PDF 处理与命令编排
- **测试**：Rust 集成测试（命令层）+ Vitest / Testing Library（mock Tauri IPC 边界）
- **语料**：`fixtures/`，纯 Python 生成，macOS PDFKit 独立验证

目标平台：macOS 12+、Windows 10 1809+ / 11 x64。

## 环境要求

- Node.js `^22.13.0` 或 `>=24`（Vite 8 / jsdom 29 要求）；npm
- Rust `>=1.90`（Tauri 2.12 的 MSRV）
- macOS 构建需要 Xcode Command Line Tools；Windows 构建需要 MSVC 工具链与 WebView2

## 快速开始

```bash
npm install
npm run tauri dev      # 启动 Vite 与 Tauri 开发窗口
```

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
| `npm test` | Vitest 单元/组件测试（mock `@tauri-apps/api/core`） |
| `npm run build` | 类型检查 + Vite 生产构建 |
| `cd src-tauri && cargo check` | Rust 编译检查 |
| `cd src-tauri && cargo test` | 命令层与 fixture 加载器测试 |
| `python3 fixtures/generate.py --check` | 校验已提交语料与生成器一致 |
| `swift fixtures/verify.swift` | 用 macOS PDFKit 独立验证每个 fixture |

## 目录结构

```
src/                  React 前端
  lib/backend.ts      Tauri 命令绑定的唯一前端入口
  App.tsx             空工作区壳
src-tauri/            Tauri / Rust
  src/commands.rs     Tauri 命令边界（当前仅 app_info）
  src/menu.rs         原生菜单（文件/编辑/视图/帮助）
  tests/              命令层与 fixture 加载器集成测试
fixtures/             固定 PDF 语料与生成/验证脚本
docs/adr/             架构决策记录
```

## 测试 seam

唯一主 seam 是 **Tauri 命令边界**：

- Rust 侧命令函数（当前 `app_info`）由 `cargo test` 直接调用，断言跨 IPC 序列化后的结果。
- 前端只通过 `src/lib/backend.ts` 调用 `invoke`；组件测试 mock 系统边界 `@tauri-apps/api/core`，不 mock 自己的模块。
- 测试不接触 PDFium/lopdf 内部类型；后续引擎测试同样通过命令边界驱动。

新增命令时：在 `src-tauri/src/commands.rs` 注册，在 `src/lib/backend.ts` 增加对应调用，测试分别驱动命令函数与 mock 后的 IPC。

## Fixture 语料

`fixtures/` 覆盖文本、扫描、加密（用户密码与所有者权限限制）、表单、结构（书签/链接/元数据/附件）、500 页大文件、旋转/混合尺寸、损坏文件。密码与生成方式见 [`fixtures/README.md`](fixtures/README.md)。

语料由 `fixtures/generate.py` 以标准库确定性生成；`fixtures/verify.swift` 用 PDFKit（与生成器不同的实现）验证加密、文本层、旋转、表单 Widget、书签/链接与截断文件行为，避免自证正确。

## 依赖许可

仅使用宽松许可依赖：Tauri、React、Vite、Vitest、Testing Library 为 MIT / Apache-2.0，TypeScript 为 Apache-2.0。仓库不含付费服务、遥测或代码签名配置。

## 当前限制

- PDF 引擎（PDFium + lopdf，ADR-0010）尚未接入；由票 03 负责。
- 界面仅为空工作区壳；M0 编辑器布局由票 02 负责。
- fixture 中的“大文件”是 500 页的提交版本；可用 `--large-pages 2000` 生成完整压力样例。
- 中文/Embedded Font/Font Subset fixture 待字体管线票补充（见 `fixtures/README.md`）。
- 生产构建目前未打包、未签名；GUI 启动冒烟需在图形会话中执行。
