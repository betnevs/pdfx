# 06: 加密 PDF 打开

**What to build:** 打开带 User Password 的 PDF 时提示输入密码；Owner Password 施加 Permission Restrictions 时进入只读模式并说明原因；密码错误有明确提示。

**Blocked by:** 04 (打开文档并渲染第一页)

**Status:** done

- [x] 带 User Password 的 PDF 打开时弹出密码输入，正确密码可进入。
- [x] 密码错误时提示重试，不泄露文档内容。
- [x] Owner Password 限制编辑时，编辑工具禁用并显示只读原因。
- [x] 只读 Document 的保存入口禁用或明确提示“另存为不加密副本”的后果。

**Verification notes:**

- 合并提交 `c9068ce`（ticket/06-encrypted-open `168704b`）。冲突：`src/App.tsx`（保留 #05 的 `useDocumentViewer`/`DocumentToolbar`/`ThumbnailRail`/`PageSurface` 结构，插入 `DocumentAccess`，保留 #06 的密码状态与 `openPath(path, password)` 流程）、`src/App.test.tsx`（以 #05 测试为底，重新插入 #06 的 4 个测试并补 `document-policy` 引用/加密 fixture 字段）、`src-tauri/src/commands.rs`（仅 PDFium prelude import 冲突）。`backend.ts`、`styles.css`、`menu.rs` 自动合并。未复活旧单体 App，导航/缩略图/全屏仍在。
- 自动验证通过：`npm run typecheck`、`npm test`（64 项）、`npm run build`、`npm run test:e2e`（2 项）、`cargo fmt --check`、`cargo clippy -- -D warnings`、`cargo test`（41 项通过，2 项手动延迟测试默认忽略）。`src-tauri/tests/encrypted_open.rs` 覆盖无密码报 `password_required`、错误密码报 `invalid_password`、User Password 打开且保持可编辑、Owner Password 解除限制、受限文档每项权限上报、受限文档仍可渲染、`DocumentView` 序列化字段；前端测试覆盖弹窗、错误后仍无任何 `render_page`/`img`、正确密码后渲染第一页、取消回到空工作区、只读原因与禁用态、加密但可编辑文档的“不加密另存”警告。
- 行为断言未弱化：密码成功后的首帧渲染断言改为 `waitFor`，因为它现在要经过 `page_info` 元数据请求（#05 架构），仍严格要求恰好一次 `render_page` 且 `pageIndex: 0, scale: 1`。
- 只读原因当前作用于 `DocumentAccess` 的编辑/保存占位入口；真正的编辑工具与保存管线分别属于 #08 与 #10，届时必须继续遵守 `readOnly`/`permissions` 与 ADR-0007 的“输出不加密”警告（规则已由 `document-policy.ts` 固化并有测试）。
- 原生 GUI 未做人工 live 测试：真实加密 fixture 的密码框输入、Owner Password 文档的实际只读呈现、系统级输入法/快捷键下的弹窗焦点仅由 Vitest mock 与 Rust 命令测试覆盖；需桌面端手工冒烟。
- 既有问题（与本 ticket 无关）：`cargo clippy --all-targets -- -D warnings` 仍因 `src-tauri/tests/support/fixtures.rs`（及 `tests/fixtures.rs` 的 `Fixture` 字段）dead_code 失败，按约定未修复。
