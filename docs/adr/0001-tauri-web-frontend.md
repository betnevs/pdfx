# 使用 Tauri 2 承载 Web 前端

产品要求 1:1 复刻 iLovePDF 编辑页的信息架构与交互密度（缩略图栏、画布、属性面板、浮动工具栏）。我们用 Tauri 2：Rust 负责 PDF 处理与文件系统，React/TypeScript 负责界面，界面跑在系统 WebView 里。纯 Rust 原生 GUI（egui/iced/slint）难以在合理工期内达到这种网页级排版与控件密度；Electron 包体过大且引入第二套运行时。
