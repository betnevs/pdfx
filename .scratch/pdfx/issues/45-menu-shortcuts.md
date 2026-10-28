# 45: 系统菜单与快捷键补齐

**What to build:** 原生菜单补齐“保存 / 另存为 / 撤销 / 重做 / 上一页 / 下一页”，并绑定不与系统冲突的快捷键；菜单项在不可用时置灰，触发后与工具栏按钮行为一致。

**Blocked by:** 10 (Save As / Save Changes 管线)

**Status:** done

- [x] 文件菜单提供“保存”“另存为”，分别使用 Cmd/Ctrl+S 与 Cmd/Ctrl+Shift+S。
- [x] 编辑菜单的撤销/重做使用 Cmd/Ctrl+Z 与 Cmd/Ctrl+Shift+Z（Windows 同时接受 Ctrl+Y），并作用于当前 Edit Session。
- [x] 视图或翻页入口提供上一页/下一页快捷键，与页码输入、缩略图选择保持同步。
- [x] 无文档或只读时相关菜单项置灰，不产生无效命令。
- [ ] 快捷键不与系统菜单、文本输入框内的原生编辑快捷键冲突；输入框聚焦时页面快捷键不触发。（输入框聚焦时菜单项禁用/释放快捷键与事件时守卫已自动化验证；真实系统菜单与原生文本撤销需人工核验，见“人工未验项”）

**Notes:** 票据 05 已实现翻页 UI 与 Home/End 处理，票据 10 提供保存命令；但原生菜单目前只有打开/设置/缩放/全屏，用户故事 22 与票据 04 的“常用快捷键”验收项仍未完成。实现时应复用前端已有的命令分发路径，不在菜单层重复业务逻辑。

**Verification (agent):** 分支 `ticket/45-menu-shortcuts`（实现 tip `6f64037`），先合入 integration tip `f744aa3`（合并提交 `685be89`，无冲突）；主仓库 `integration/pdfx` 以 `git merge --no-ff` 合入为合并提交 `cc4e65f`（无冲突）。合并后独立审计发现并修复两处真实缺陷，修复提交 `a5abf19`；关闭提交为 `chore: close ticket 45`（即本文件的最后一次修改）。

相对 integration tip `f744aa3` 的代码改动为 15 个文件（+1317/−67，含审计修复的 +182/−15）：`src-tauri/src/lib.rs`、`src-tauri/src/menu.rs`、`src/App.tsx`、`src/App.test.tsx`、`src/components/PageTextLayer.tsx`、`src/components/PageTextEditing.test.tsx`、`src/lib/backend.ts`、`src/lib/backend.test.ts`、`src/lib/keyboard.ts`、`src/lib/keyboard.test.ts`、`src/lib/menu.ts`、`src/lib/menu.test.ts`、`src/lib/useDocumentSave.ts`、`src/lib/useDocumentViewer.ts`、`src/lib/useDocumentViewer.test.tsx`；另更新了本票文件。未改 `src-tauri/vendor`（vendor diff 为空，`PDFX-PATCH.md` 仍为 10 个 close 类型）、`Cargo.toml`/`Cargo.lock`/`package.json`/`package-lock.json`、spec、ADR、`tauri.conf` 或 capabilities，未新增依赖，无网络/遥测。

**实现：** `src-tauri/src/menu.rs` 用 `MenuBuilder` 补齐 文件/编辑/视图/帮助 四个子菜单（macOS 另有应用菜单），菜单项为中文：打开…、打印…、导出当前页…、保存、另存为…、设置…、撤销、重做、实际大小、放大、缩小、适合宽度、适合页面、全屏、上一页、下一页、关闭窗口、退出 PDFX；剪切/复制/粘贴/全选使用 Tauri 预定义项。16 个受状态控制的 id 与 `accelerator_for` 集中在一张表：保存 `CmdOrCtrl+S`、另存为 `CmdOrCtrl+Shift+S`、撤销 `CmdOrCtrl+Z`、重做 `CmdOrCtrl+Shift+Z`、上一页 `PageUp`、下一页 `PageDown`，其余为打开/打印/导出/设置/缩放/全屏快捷键（全屏 macOS `Ctrl+Command+F`，其他 `F11`）。前端 `src/lib/menu.ts::menuItemStates` 是唯一可用性映射；`App.tsx` 在签名变化和窗口重新获得焦点时通过 `src/lib/backend.ts::setMenuState` 这一条命令缝推送到 `set_menu_state`，菜单事件只经 `handleMenu` 分发到已有命令/toolbar 路径，不在菜单层复制业务逻辑。

**独立审计（合并后）：** 在 `cc4e65f` 上独立复核并新增前端 3 项、Rust 2 项用例，据此定位并修复两处真实缺陷。

1. **菜单状态从未生效（严重）：** 原 `set_menu_state` 用 `Menu::get(&item.id)` 查项，而 Tauri 2.12.5 的 `Menu::get` 只遍历菜单栏的直接子项（即 文件/编辑/视图/帮助 子菜单），受状态控制的 16 个项都在子菜单下一层。因此每次状态推送都会在第一项返回 “menu item … is missing”，原生菜单永远停留在 `initial_enabled`（只有打开/设置/全屏可用），保存、撤销、翻页等快捷键与置灰全部失效；分支自带测试只测前端快照与 Rust 表，未覆盖真实菜单树，故未暴露。修复为 `find_item` 递归遍历 `menu.items()`/`submenu.items()`（修复提交 `a5abf19`）。该查找无法在不上新增 dev-dependency 的前提下单测（构造原生菜单需要 Tauri `test` feature/主线程），本结论由 Tauri 2.12.5 源码（`Menu::get` 仅查 `self.0.items`，子菜单项在 `MenuInner.items`）与编译/门禁验证；菜单表本身有唯一性测试。
2. **输入框聚焦竞态：** 菜单项禁用快照要经过一次异步命令往返才到达原生菜单；在此窗口内到达的加速键事件会绕过 React 状态，导致输入框刚聚焦时 PageUp/PageDown 翻页、Cmd/Ctrl+Z 撤销文档。修复在 `handleMenu` 的事件时点增加 `document.activeElement` 守卫：`edit.undo`、`edit.redo`、`page.prev`、`page.next` 在可编辑控件聚焦时直接返回（`a5abf19`）。新用例 “ignores page and history menu events that race ahead of the focus snapshot” 在修复前失败（页码变为 2、`undo_text_edit` 被调用），修复后通过。
3. **设置对话框未冻结菜单：** 审计补充 `modalOpen` 包含 `settingsOpen`，设置对话框打开时打开/保存/设置/翻页等菜单项全部禁用，并新增用例 “freezes the menu while the settings dialog is open”。
4. **其余审计补充：** Rust 用例覆盖禁用时释放加速键、启用时恢复同一加速键（`disabling_an_item_releases_its_accelerator_and_enabling_restores_it`）与 16 项加速键唯一性（`no_two_stateful_items_share_an_accelerator`）；前端新增 “does not treat Ctrl+Y as redo outside Windows”，确认平台门控。

**逐项行为（独立验证）：**

1. **保存/另存为：** 菜单 id `file.save`/`file.save_as` 分别使用 `CmdOrCtrl+S`/`CmdOrCtrl+Shift+S`，`handleMenu` 调用 `useDocumentSave` 的 `saveChanges`/`saveAs`，完全复用票据 10 管线（首次保存走 Save As、加密文档未加密副本确认、目标路径校验、保存中禁用）。既有用例 “wires the file.save and file.save_as menu ids to the save flow” 及保存/关闭套件通过；打开、打印、导出菜单项分别调用文件对话框与 `PrintExport` 的 `openPrintDialog`/`openExportDialog`，与 toolbar 按钮同一函数（用例 “opens print and export from the toolbar and the native menu”）。16 个受控项无重复加速键（Rust 测试），预定义剪切/复制/粘贴/全选/关闭/退出/隐藏项与自定义键不冲突（源码核对）。
2. **撤销/重做单一 Edit Session 路径：** 菜单 `edit.undo`/`edit.redo` → `App.runHistoryAction` → 后端 `undo_text_edit`/`redo_text_edit`，随后 `acceptHistory` 更新 canUndo/canRedo/busy 并发 `notifyDocumentChanged` 刷新页面位图、覆盖层与未保存标记；`historyActionInFlightRef` 防止重入。`PageTextLayer` 的 keydown 已删除 Cmd/Ctrl+Z 处理（源码注释明确），全仓库键盘侧只有菜单与 Windows Ctrl+Y 回退，工具栏按钮是两个覆盖层自己的 UI 入口，不构成第二次键盘分发；用例 “runs menu undo and redo once against the unified edit history” 断言一次按键/一次菜单事件只调用一次。Windows Ctrl+Y 仅由 App 的 window keydown 提供，条件为 `isWindowsPlatform()`、Ctrl 且无 Meta/Alt/Shift、非可编辑目标、`edit.redo` 当前可用；`edit.redo` 本身不注册 Ctrl+Y（`redo_does_not_register_a_second_windows_accelerator`），因此不会双触发；非 Windows 用例确认 Ctrl+Y 不动作，macOS 仍由菜单 `Cmd+Z`/`Cmd+Shift+Z` 承担。文本输入聚焦时撤销/重做项禁用并释放加速键，且事件时守卫生效，textarea 保留平台原生文本撤销。
3. **翻页：** 菜单 `page.prev`/`page.next` 使用 `PageUp`/`PageDown`，二者都调用 viewer 唯一的 `goToPage`（夹取到 0..pageCount-1），页码输入与缩略图同样传 `viewer.goToPage`（`onSelectPage`），共享同一 `pageIndex`；首页/末页时对应项禁用，打开/关闭文档与页码变化都会更新快照；票据 05 的 Home/End 仍由视口 keydown 处理并保留可编辑目标守卫。全仓库没有其他 PageUp/PageDown handler；查看器是单页模式，PageUp/PageDown 在页面放大时本来只用于页内滚动，作为翻页快捷键是票据 04/用户故事 22 的约定，已记录取舍。muda 0.20.0 将无修饰的 `PageUp`/`PageDown` 映射为 macOS NSFunctionKey `\u{F72C}`/`\u{F72D}`、Windows `VK_PRIOR`/`VK_NEXT`、GTK `Page_Up`/`Page_Down`；真实 AppKit 菜单键等价物与 WKWebView 滚动的优先级需人工核验。
4. **禁用状态：** `menuItemStates` 规则独立测试覆盖：无文档时文档相关项全禁用（打开/设置/全屏可用）；只读时保存/撤销/重做禁用而另存为可用（权限限制语义）；无历史时撤销/重做禁用；`saving` 时保存与历史动作禁用；关闭确认、密码、保存确认、设置对话框打开时打开/设置/全屏与文档动作禁用；文本输入聚焦时撤销/重做/上一页/下一页禁用；首页/末页边界。`handleMenu` 先用 `menuEnabledRef` 拒绝禁用命令，阻止“事件先到、状态后到”的无效命令。Rust 侧 `validate_menu_state` 在触碰任何菜单项前拒绝未知 id（`an_unknown_menu_id_is_rejected_before_any_item_changes`），`set_enabled` 与加速键释放/恢复分别有测试；原生置灰外观见人工项。
5. **输入焦点与系统冲突：** `focusin`/`focusout` 切换 `textInputFocused`，键盘可用性快照与事件时 `document.activeElement` 双层守卫；可编辑目标覆盖 input/textarea/select/contentEditable（`isEditableTarget` 有独立测试），因此焦点进入输入框后 PageUp/PageDown 与撤销/重做不触发，移出后恢复（焦点测试覆盖 focus→blur→视口）。`PageTextLayer` 的 textarea 在编辑状态下直接返回、不拦截按键；应用层不监听 Cmd/Ctrl+Z，文本控件内保持 WebView 原生撤销。真实系统菜单加速键捕获与原生文本撤销见人工项。

**线程与多窗口：** `set_menu_state` 是同步 `#[tauri::command]`（默认 `ExecutionContext::Blocking`），菜单修改经 Tauri 菜单包装器投递主线程：`send_user_message` 在已在主线程时内联执行，否则代理到主线程，因此同步命令不会阻塞等待主线程造成死锁；错误以 `CommandError` 返回，无 panic 路径。结论由 tauri 2.12.5 / tauri-runtime-wry 2.12.3 / muda 0.20.0 源码核对，无法在不新增 `test` feature 的情况下自动化。原生菜单是应用级（app-wide）：两个文档窗口对可用性可能有不同快照，后台窗口的推送会短暂覆盖前台窗口菜单；前端在窗口 `focus` 时重推快照缓解，`on_menu_event` 只向聚焦窗口发送事件，`handleMenu` 再用本窗口快照与焦点守卫拒绝不适用的命令。瞬时不一致是 app-wide 菜单的固有限制，已记录。

**门禁（合并 + 审计修复后，主仓库 `a5abf19`）：** `npm run typecheck` 通过；`npm test` **39 文件 / 452 项**通过（合并后基线 449 项 + 审计新增 3 项）；`npm run build` 通过；`npm run test:e2e` **12 项**通过；`cargo fmt --check` exit 0；`cargo clippy -- -D warnings` exit 0（仅 vendored `pdfium-render` 既有 4 条 warning，未用 `--all-targets`）；`cargo test` **313 通过 / 0 失败 / 3 ignored**（分支报告 306 项；合并 integration tip 带来的测试加审计新增 2 项 Rust 测试后为 313，其中 `src-tauri/src/menu.rs` 10 项）。关键用例：`runs menu undo and redo once against the unified edit history`、`keeps page and history shortcuts out of focused text controls`、`ignores page and history menu events that race ahead of the focus snapshot`、`accepts Ctrl+Y for redo on Windows without a second menu accelerator`、`does not treat Ctrl+Y as redo outside Windows`、`freezes every menu item while the close question is open`、`freezes the menu while the settings dialog is open`、`re-publishes the app-wide menu when the window regains focus`、`wires the file.save and file.save_as menu ids to the save flow`、`opens print and export from the toolbar and the native menu`；Rust：`disabling_an_item_releases_its_accelerator_and_enabling_restores_it`、`no_two_stateful_items_share_an_accelerator`、`redo_does_not_register_a_second_windows_accelerator`、`an_unknown_menu_id_is_rejected_before_any_item_changes`。

**人工未验项（勾选项 5 未勾选）：** 真实 macOS/Windows 打包应用中的原生菜单外观、中文标签与加速键显示/捕获未人工核验；重点包括无修饰 `PageUp`/`PageDown` 作为 AppKit 菜单键等价物与 WKWebView 页内滚动的实际优先级、真实输入框聚焦时系统原生文本撤销（Cmd+Z）不被菜单拦截、真实 Windows Ctrl+Y 路径，以及多窗口/后台窗口推送造成的瞬时菜单状态。自动化仅覆盖 jsdom 前端分发与状态快照、Rust 菜单表/校验逻辑，以及 Tauri/muda 源码核对。

**已知限制/后续：** 原生菜单为 app-wide，后台窗口的可用性推送可能瞬时覆盖前台窗口（聚焦重推已缓解，未做逐窗口菜单）；`PrintExport` 的打印/导出对话框状态在组件内部，未纳入 `App` 的 `modalOpen`，其打开期间菜单不会整体冻结（对话框自身仍按既有逻辑守卫命令）；单页查看器下 `PageUp`/`PageDown` 被定义为翻页，页面放大时不再承担页内滚动；Rust 递归查找菜单项无单测（需 Tauri `test` feature/新增 dev-dependency），已用源码核对与构建门禁覆盖。
