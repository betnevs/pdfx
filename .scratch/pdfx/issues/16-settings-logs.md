# 16: 设置、持久化与日志

**What to build:** 设置页提供界面语言、主题、Annotation 显示名、文件关联开关与更新检查入口；设置本地持久化；应用写本地滚动日志，用户可手动导出诊断包。

**Blocked by:** 04 (打开文档并渲染第一页)

**Status:** done

- [x] 设置修改后立即生效并在重启后保留。
- [ ] 文件关联开关能注册/取消 PDFX 为系统 PDF 打开方式（macOS/Windows 各自实现）。
- [ ] Annotation 显示名修改后，新建 Annotation 使用新作者名。
- [ ] 本地滚动日志包含打开、保存、错误等关键事件；诊断包由用户手动导出，不自动上传。
- [x] 设置页不包含遥测开关或任何自动上传选项。

**Verification notes:**

- 合并提交 `afd0012`（`ticket/16-settings-logs` `aa39f0b`，feature `aaac0bc`，merge base `2850770`），`git merge --no-ff`。仅 `src-tauri/src/lib.rs` 冲突：同时保留 `settings` 与 `text_edit` 模块及两组 command 注册（`get_settings`/`update_settings`/`export_diagnostics` 与 text edit 的 12 个命令）。预期冲突（`src/App.tsx`、`src/lib/backend.ts`、`src/lib/useDocumentViewer.ts`、`src/styles.css`）均未发生：ticket 16 基于 `2850770`、08/09 在其后落地，但改动文件不重叠，`App.tsx`/`App.test.tsx` 由 git 自动合并；合并后设置、主题、密码流程、导航/缩略图、文字选择/编辑与保存 UI 的前后端测试全部通过。未修改 spec、ADR、打包配置或其他 ticket。
- 勾选 1：`SettingsStore::save` 采用同目录临时文件 + `sync_all` + rename 原子写入（`saving_is_atomic_and_leaves_no_temporary_files` 断言无临时文件残留）；`settings_survive_a_restart_through_a_new_store` 断言重新构造 store 后语言/主题/作者/文件关联全部保留；缺失、损坏、字段缺失的 `settings.json` 均回退默认值。前端 `useSettings` 在加载与每次保存后用后端返回值调用 `applySettings`：主题写入 `:root[data-theme]`（`src/settings.css` 提供深色 token；`system` 时移除属性并由 `prefers-color-scheme` 接管），语言写入 `<html lang>`；设置面板除显示名（失焦/回车提交）外均即改即存。已知限制：界面文案尚未国际化，选择 English 只改变 `lang` 属性，可见文案仍为中文；当前无 i18n 资源或后续 ticket，建议单独排期。
- 勾选 2 未验证：实现已存在但真实 OS 行为未验证。macOS 通过 `/System/.../lsregister -f|-u <PDFX.app>` 注册/移除，bundle 由 resource dir 推导，非打包构建返回 `unsupported` 并给出 Finder“显示简介”指引；Windows 以 `HKCU\Software\Classes\PDFX.Document` + `.pdf\OpenWithProgids` 注册并只删除自己的项，不写 `.pdf` 默认值（避免与系统默认应用哈希保护冲突）。两端成功注册均返回 `user_action_required` 并说明用户仍需在系统默认应用 UI 确认；平台失败保留偏好并返回 `failed`。自动化只覆盖 mock 平台、unsupported 路径与（仅在 Windows 上编译的）命令构造；真实 LaunchServices / 注册表往返仅由 `#[ignore]` 的 `manual_real_platform_registration_round_trip` 手动执行，本次未在打包应用上运行。
- 勾选 3 未验证：设置项已持久化并校验（trim、非空、≤64 字符、禁止控制字符），`SettingsUpdateResult` 与前端 `Settings.annotationAuthor` 已作为契约暴露；但当前集成分支尚无 Annotation 创建功能（ticket 12/13 未合并），无法断言“新建 Annotation 使用新作者名”。ticket 12/13 需从 settings 读取该字段作为作者。
- 勾选 4 部分验证、留空：打开/关闭/错误日志与诊断包已验证，保存事件缺失。`Logger` 写入 `app_log_dir()/pdfx.log`（1 MB 软上限、保留 3 个滚动文件、UTC 毫秒时间戳、每个事件一行、换行转义）；已接入 `app.start`、`engine.unavailable`、`document.open`（名称/页数/路径）、`document.open.failed`（代码/消息）、`document.close`、`settings.update`（刻意不记录作者名）、`diagnostics.export`。`export_diagnostics` 只把版本、平台与全部保留日志拼接为文本，写到用户通过原生保存对话框选择的路径；源码中没有 HTTP/网络调用，设置页也无自动上传入口。保存管线属于 ticket 10（未合并，当前无 save 命令或 UI），故 `document.save` 事件尚未接入；ticket 10 应在实际写盘成功处记录该事件。
- 勾选 5：设置面板只有一个复选框（文件关联），`SettingsPanel.test.tsx` 断言仅一个 checkbox、无“遥测”文案、无自动上传项；“检查更新”为禁用占位按钮，真实检查由 ticket 19 实现。诊断导出为显式用户操作。
- 依赖/许可审查：本 ticket 未新增任何 Cargo 或 npm 依赖（`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、`package.json`、`package-lock.json` 相对 `6526771` 无变化），仅复用既有 `serde`/`serde_json`/`tauri`/`tauri-plugin-dialog`，无新增许可证或 advisory。`dialog:default` 已包含 `allow-save`，诊断导出所需的保存对话框权限无需改配置。
- 打包配置审查：`src-tauri/tauri.conf.json`、`src-tauri/tauri.macos.conf.json`、`src-tauri/tauri.windows.conf.json`、`src-tauri/capabilities/default.json`、`scripts/` 均未修改。
- 自动化验证（合并后于仓库根执行）：`npm ci`（115 包，0 漏洞）、`npm run setup:native`、`npm run typecheck`、`npm test`（16 文件、135 项）、`npm run build`、`npm run test:e2e`（2 项）全部通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 的 4 条 dead_code warning）；`cargo test` 104 项通过、3 项手动/延迟测试忽略（file association 真实平台往返、navigation 与 open_render 延迟测量），其中 settings 13、logging 7、file_association 2。
- 已知既有问题（与本 ticket 无关，按约定不修）：`cargo clippy --all-targets -- -D warnings` 仍因 `tests/support/*` 的 dead_code 失败；本 ticket 新增的 `tests/support/temp.rs` 也属于同一模式（仅部分测试二进制使用其辅助项）。
- 后续：
  - ticket 10（保存管线）接入 `document.save` 日志，并在成功写盘后调用 `mark_document_saved`。
  - ticket 12/13（Annotation）创建时使用 `settings.annotationAuthor`。
  - ticket 19 用真实“检查更新”替换禁用占位按钮；启用后仍须用户主动触发、无遥测。
  - 发布前在真实 macOS 打包应用与 Windows 构建上手动执行 `cargo test --test file_association -- --ignored`，并验证 Finder/资源管理器的默认应用确认流程。
  - 界面语言若要真正切换文案，需要单独的 i18n 资源与文案迁移。
- 未验证的人工 GUI 项：设置面板在真实 Tauri 窗口中的保存状态提示、原生保存对话框导出诊断包、主题切换在编辑器全界面的视觉效果；自动化仅通过 mock `invoke`/`plugin-dialog` 与 CSS token 断言覆盖。
