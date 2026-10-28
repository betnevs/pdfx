# 47: 已有表单字段的填写保真

**What to build:** 填写已有 Form Field（含嵌套/层级字段）后，字段值、选中状态与外观流同步更新，使其他阅读器显示新值；保存、撤销/重做与复制 Page 后保持一致。本条修复 ticket 39 审计确认的既有缺陷，不新增字段类型。

**Blocked by:** 30 (填写表单字段), 31 (创建文本字段与外观流), 33 (选择/签名字段), 39 (复制 Page)

**Status:** ready-for-agent

- [ ] 终端字段与 `/Kids` 嵌套、`/Parent` 层级字段都能填写；值可被 PDFium 与独立 lopdf 读取，界面与保存结果一致。
- [ ] 每个被填写的 Widget 都得到与新值一致的显式 `/AP /N`（CJK 使用内置 Noto Sans SC 子集，按页面 `/Rotate` 补偿），不依赖 `/NeedAppearances`；保存后 PDFium 渲染像素与读取到的值一致，旧的源码外观不再残留。
- [ ] 复选框/单选的 `/AS` 与 `/V` 一致；列表/下拉的 `/I` 与 `/V` 一致；只读文档拒绝填写且不改变历史。
- [ ] 填写、撤销、重做与重复保存保持单会话历史；每次保存最多一个 lopdf revision；重复保存不重复生成字体或外观对象；Source Document 不变。
- [ ] 复制 Page 得到的字段与 Widget 在填写后显示新值，且与源 Page 的字段身份独立（不共享 `/NM`、字段名或外观对象）。
- [ ] 测试使用固定表单 fixture 覆盖上述路径，并以独立 lopdf 解析 + PDFium 固定 DPI 像素断言验证外观，而不是只断言 `/V`。

**Notes:** ticket 30 当前只更新 `/V`、`/AS`、`/I`，已有 Widget 保留源文件中的 `/AP /N`；ticket 39 审计复现了“填写后 PDFium/其他阅读器仍显示旧值”，并确认原始字段与复制字段行为相同，属 ticket 30 的既有缺陷而非复制逻辑。嵌套源字段（`/Kids` 中的 Widget）填写为静默 no-op。ticket 31 已具备 Noto Sans SC 子集、显式 `/AP /N`、UTF-16BE 与 `/Rotate` 补偿的写入机制，应抽取复用，不要另建一套外观生成器。填写现有字段仍不执行计算或 JavaScript。
