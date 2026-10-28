# 47: 已有表单字段的填写保真

**What to build:** 填写已有 Form Field（含嵌套/层级字段）后，字段值、选中状态与外观流同步更新，使其他阅读器显示新值；保存、撤销/重做与复制 Page 后保持一致。本条修复 ticket 39 审计确认的既有缺陷，不新增字段类型。

**Blocked by:** 30 (填写表单字段), 31 (创建文本字段与外观流), 33 (选择/签名字段), 39 (复制 Page)

**Status:** done

- [x] 终端字段与 `/Kids` 嵌套、`/Parent` 层级字段都能填写；值可被 PDFium 与独立 lopdf 读取，界面与保存结果一致。
- [x] 每个被填写的 Widget 都得到与新值一致的显式 `/AP /N`（CJK 使用内置 Noto Sans SC 子集，按页面 `/Rotate` 补偿），不依赖 `/NeedAppearances`；保存后 PDFium 渲染像素与读取到的值一致，旧的源码外观不再残留。
- [x] 复选框/单选的 `/AS` 与 `/V` 一致；列表/下拉的 `/I` 与 `/V` 一致；只读文档拒绝填写且不改变历史。
- [x] 填写、撤销、重做与重复保存保持单会话历史；每次保存最多一个 lopdf revision；重复保存不重复生成字体或外观对象；Source Document 不变。
- [x] 复制 Page 得到的字段与 Widget 在填写后显示新值，且与源 Page 的字段身份独立（不共享 `/NM`、字段名或外观对象）。
- [x] 测试使用固定表单 fixture 覆盖上述路径，并以独立 lopdf 解析 + PDFium 固定 DPI 像素断言验证外观，而不是只断言 `/V`。

**Notes:** ticket 30 当前只更新 `/V`、`/AS`、`/I`，已有 Widget 保留源文件中的 `/AP /N`；ticket 39 审计复现了“填写后 PDFium/其他阅读器仍显示旧值”，并确认原始字段与复制字段行为相同，属 ticket 30 的既有缺陷而非复制逻辑。嵌套源字段（`/Kids` 中的 Widget）填写为静默 no-op。ticket 31 已具备 Noto Sans SC 子集、显式 `/AP /N`、UTF-16BE 与 `/Rotate` 补偿的写入机制，应抽取复用，不要另建一套外观生成器。填写现有字段仍不执行计算或 JavaScript。

**Verification notes:**

在 `integration/pdfx` 上独立审计并关闭（合并提交 `bc17421`；审计修复 `9dcb465`；加密源测试 `bc371be`）。所有验收项均以命令边界测试 + 独立 lopdf 解析 + PDFium 固定 DPI 像素断言验证，未仅断言 `/V`。

逐项结果：
1. 终端字段、`/Kids` 终端字段（`membership_id`）、`/Parent` 嵌套字段（`person.name`）与复制 Page 字段均可填写；PDFium 与独立 lopdf 读回值一致，保存后界面与文件一致。
2. 每个被填写 Widget 均有显式 `/AP /N`：共享 Noto Sans SC 子集（Type0/Identity-H + FontFile2）、UTF-16BE CJK、按页面 `/Rotate` 补偿（像素断言）；无 `/NeedAppearances`；源文件中的红色 STALE 外观不再被任何新 AP 引用。
3. Checkbox 的 `/V` 与 `/AS` 一致（含 Off）；radio 选中项为对应 on-state、其余 Off；choice 写入 `/I` 并以所选标签重建外观。字段级 ReadOnly、文档级权限限制与签名字段均拒绝填写，且拒绝发生在任何历史变更之前。
4. 撤销后保存移除外观并恢复 `/V`，重做同时恢复两者；单会话历史。每次保存最多一个 lopdf revision（输出共 3 个 `startxref`/`%%EOF`）；重复保存对象画像稳定（1 个 Type0、1 个 FontFile2、19 个 Form XObject，`/DR` 仅 Helv + 单个 PdfxNoto）；Source Document 字节不变。
5. 复制 Page 字段名为 `__copy_pN` 后缀、`/NM` 重命名、`/AP` 对象独立，源 Page 的值与对象不变。
6. 固定 fixture `form-fill-appearance` 覆盖上述路径；新增 `src-tauri/tests/form_fill_robustness.rs` 7 个回归测试与 `form_fill_appearance.rs` 的加密源测试。

审计中发现并修复的真实缺陷（`9dcb465`）：
- 填写字段后删除其所在 Page 再保存会失败（文本/复选与选择字段均可复现）：`/Fields` 剪枝先移除字段，历史回放仍写入。现在 `applied_values` 按 `PageModel.order` 过滤已删除页的动作；删除后撤销恢复 Page 时填写值仍会写回。
- 畸形 `/Kids` 环导致保存期字段遍历无限递归并 `stack overflow`（SIGABRT）。save 端遍历与 PDFium 可写性探测均加入 visited 集合与深度上限 64。
- 重名（重复 `/T`）以及字面点号名与嵌套路径同名（`a.b`）时，`/V` 被写到错误字段对象，被点击 Widget 保留旧值与旧 AP。现在按全名返回所有终端字段并逐一写入 `/V`、`/AS`、`/I`；`form_create` 为每个匹配重建外观；同名多对象时绕过 PDFium setter 走保存 overlay。choice 的 `/Opt` 查找与外观生成统一使用继承链。
- 缺失 `/Rect`：不 panic，值可正常填写与保存（该 Widget 不生成外观）；继承 `/FT`/`/Ff`/`/DA`：PDFium 识别到 Text 类型、multiline 标志与继承的 18 Tf，外观多行基线 63.6 = 80 − 2 − 18×0.8，均有回归测试。

加密源（新增测试）：AES-256 `/V 5` revision 6 fixture，填写 `/Kids` 终端字段与多行合并字段后以新密码保存；解密输出断言 `/V`、全新的 `/PdfxNoto` 外观（非 STALE）、无 `/NeedAppearances`；输出保持加密、旧源密码被拒绝、源文件字节不变，PDFium 用新密码重新打开后值一致。

最终门禁：`npm run typecheck` 通过；`npm test` 51 个文件 626 个测试全通过；`npm run build` 通过；`npm run test:e2e` 12 个通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` 通过（仅 vendored `pdfium-render` 的既有 warning）；`cargo test` 488 通过 / 0 失败 / 3 ignored（40 个测试二进制）。

剩余人工项：无阻塞项。可选：在 Acrobat/Preview 中对含旋转与 CJK 的输出做一次跨阅读器抽查（自动化已用 PDFium 固定 DPI 像素断言覆盖）。
