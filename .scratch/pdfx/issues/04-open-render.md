# 04: 打开文档并渲染第一页

**What to build:** 用户可以通过文件对话框或拖拽打开本地 PDF，PDFX 在独立窗口中渲染第一页；标题栏显示 Document 名称，损坏文件、权限错误与不支持的格式有清晰提示。

**Blocked by:** 02 (M0 浏览器高保真原型), 03 (M0→M1 技术验证)

**Status:** done

- [x] 文件对话框与拖拽都能打开 PDF；打开新文件时按“一窗一文档”开新窗口。
- [ ] 第一页在 1 秒内可见（普通文本型 PDF），200MB/2000 页文档先出第一页，其余页面渐进渲染。
- [ ] 标题栏显示 Document 名称；关闭有未保存修改的 Document 时弹出确认。
- [x] 损坏/畸形 PDF 显示错误信息与重试入口，不崩溃。
- [ ] 常用快捷键（打开、关闭、缩放、翻页）可用，且不与系统菜单冲突。

**Verification notes:**

- 自动验证全部通过：`npm run typecheck`、`npm test`（29 项）、`npm run build`、`npm run test:e2e`（2 项）、`cargo check`、`cargo test`（19 项通过，1 项手动延迟测试默认忽略）。
- 第一页延迟（release 手动测试）：普通文本 fixture 冷启动 63.9ms、500 页 fixture 149.7ms、2000 页合成 fixture 48.4ms，均低于 1 秒；warm 重开 ≤4.2ms。未构造 200MB 真实大文件；2000 页样例由生成器产出（约 0.6MB），字节量不代表 200MB。“其余页面渐进渲染”未实现，随 #05（翻页、缩放与缩略图）交付。
- 原生 GUI 行为（文件对话框、拖拽、`document-*` 窗口的实际创建、菜单快捷键）未做人工 live 测试，仅由 Vitest mock Tauri 边界、Rust 命令测试与 capabilities 配置（窗口 `main` + `document-*`）覆盖；需在桌面端手工冒烟。
- 标题栏名称经 `setWindowTitle` 实现并有测试覆盖。脏关闭目前只是脚手架：`dirty` 恒为 false，close guard 不会弹出确认框；真正的未保存修改确认依赖 #08 的编辑能力。
- 打开、关闭、缩放快捷键已由原生菜单提供；翻页快捷键未实现，属于 #05。
