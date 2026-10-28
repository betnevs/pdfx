# 08: 编辑已有文字 + 撤销/重做

**What to build:** 用户点击已有文字进入 Text Box 编辑，保留原 Text Object 的字体、字号、颜色与位置；修改后移除原文字并写入新文字，支持删除 Text Box 与统一撤销/重做。

**Blocked by:** 07 (文本提取与文本层)

**Status:** ready-for-agent

- [ ] 点击文字进入编辑态，Text Box 的初始样式与位置来自原 Text Object。
- [ ] 修改文字后画布立即反映；删除 Text Box 时原文字真正移除，复制粘贴不出现重复文字。
- [ ] 不做段落 reflow；超长文字按 Text Box 宽度换行并向下扩展。
- [ ] 所有文本编辑动作可撤销/重做，保存后仍可撤销到保存前。
- [ ] Scanned PDF 上禁用文字编辑并显示原因。
