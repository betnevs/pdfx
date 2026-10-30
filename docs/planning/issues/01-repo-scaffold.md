# 01: Repo + Tauri/React 脚手架 + 测试与 fixture 骨架

**What to build:** 一个可启动的 PDFX 桌面应用骨架：Tauri 2 窗口加载 React 界面，原生菜单与标题栏就位；测试命令与固定 fixture 目录可用。后续每张票都在这个骨架上交付，不另开测试 seam。

**Blocked by:** None (can start immediately)

**Status:** done

- [x] `cargo check`、前端类型检查与测试命令在干净环境可运行。
- [x] 应用启动后显示 PDFX 空工作区，原生标题栏与菜单可见。
- [x] fixture 目录与加载器可用，至少包含文本型、Scanned、加密、表单、结构、大文件、旋转与损坏 PDF 的最小样例或生成脚本。
- [x] 测试只通过 Tauri 命令边界或前端公开接口驱动，不直接依赖 PDFium/lopdf 内部类型。
