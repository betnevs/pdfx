# 38: 图层

**What to build:** 用户可以在图层面板查看 Page 的图层，切换可见性，重命名或删除可选图层；保存后图层状态保留。

**Blocked by:** 10 (Save As / Save Changes 管线)

**Status:** done

- [x] 图层面板列出当前 Page 的 Optional Content Groups 与可见性状态。
- [x] 切换可见性立即反映在画布；保存后状态保留。
- [x] 可重命名图层；删除图层前有确认，并说明对内容的影响。
- [x] 没有图层的 Document 显示空状态，不报错。
- [x] 撤销/重做覆盖可见性与重命名操作。

**Verification notes:**

审计结论：功能已按验收标准实现，并在 ticket 分支上修复了两处真实缺陷后合入。第一条验收标准实现为**文档级**列出 `/OCProperties /D /OCGs` 中的全部 Optional Content Group（名称解码、默认可见性），不按页过滤；这是经审计确认的可记录偏差（面板列出的是文档图层，页面专属过滤未实现，测试与 UI 文案都不声称按页过滤）。空文档或零图层时面板显示中文空状态且不报错。

已修复缺陷：
1. 复制页面后、在任何图层编辑之前保存：PDFium 会为复制页克隆 OCG 字典，旧实现只在有图层编辑时映射别名，导致保存后隐藏图层的可见性丢失。现在保存时只要存在被包含的复制页就执行别名重映射（`ObjectEdit::LayerAliases`），画布渲染在存在复制页时强制使用 shadow 文档，保证即时预览与保存结果一致。
2. 删除图层后，旧实现仍从页面 `/Resources /Properties` 与 `/Configs` 引用被删 OCG。现在资源属性改写会移除已删除（含别名解析后命中）的引用，`/Configs` 中直接字典被过滤、被引用配置字典被克隆后清理，`/D` 中未知键内的已删引用被深度清理。回归测试用独立 lopdf 遍历器从 trailer 验证被删 OCG 对象不可达。

删除拒绝范围：OCMD `/P` 或 `/VE`、注释 `/OC`、XObject `/OC`、不安全的表单 XObject、未配平的 marked content 均返回 `layer_delete_unsupported`，不修改文档、不写历史、面板不变。成功删除会从 `/OCGs`、`/ON`、`/OFF`、`/Order`、`/RBGroups`、`/Configs`、页面属性及渲染内容中移除该图层，`/AS` 被丢弃，`/BaseState /OFF` 与显式 `/ON`/`/OFF` 正确处理，未知 `/D` 键（如 `/ListMode`）保留；重复保存稳定，源文件字节不变，加密源可另存为加密或未加密输出，删除/隐藏后页面操作与缩略图/区域渲染/导出/打印均一致。重命名会 trim、拒绝空白/控制字符/超 120 字符，可撤销重做，UTF-16BE 编码，保存后保留。可见性与删除均支持撤销/重做；通用撤销/重做后面板会刷新。只读或权限受限文档显示中文说明并禁用控件，后端也会拒绝且不改历史。

已知限制（已在测试中体现，未修复）：文本提取与文本层仍包含隐藏图层的文本，因为 PDFium 的文本提取器忽略 OCG 可见性；若在提取阶段过滤，会与使用活动文档映射字符索引的 `text_edit_target` 错位，导致文本编辑命中错误对象，故保持现状。同名（`/Name` 相同）的复制页别名无法唯一映射时保持原样、不猜测。

门禁（合入 `integration/pdfx` 后，merge commit `3d6b930`，工作树与 ticket tip 一致）：`npm run typecheck` 通过；`npm test` 52 个文件、649 个测试全通过；`npm run build` 通过；`npm run test:e2e` 12 个通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` 通过（仅有 4 条既有 vendored `pdfium-render` dead-code 警告）；`cargo test` 534 通过、0 失败、3 忽略（忽略项均为人工/延迟测量测试）。图层测试套件 17/17 通过。

未勾选的人工项：真实 PDF 阅读器中的渲染一致性、真实桌面环境下打印/导出与只读权限提示的人工检查未执行；自动化测试覆盖了 PDFium 像素解码、lopdf 结构与前端交互，但不等同于人工验收。
