# 35: Flatten 注释导出

**What to build:** 用户可以把视觉 Annotation（高亮、下划线、删除线、便签、手绘、形状、图章、高级注释）Flatten 成页面内容的副本；链接与 Form Field 保持可交互。

**Blocked by:** 12 (文本标记注释与注释列表), 13 (便签与手绘注释), 20 (矩形与椭圆形状), 25 (图章), 34 (高级注释)

**Status:** ready-for-agent

- [ ] 导出副本中视觉 Annotation 变成页面内容，任何阅读器显示一致。
- [ ] 链接与 Form Field 不被 Flatten，仍可点击/填写。
- [ ] 原 Document 不被修改；Flatten 只写新副本。
- [ ] 便签等无页面外观的注释有明确处理（渲染图标或省略）。
- [ ] 导出结果在 Preview、Adobe Acrobat Reader、Chrome 中验证。
