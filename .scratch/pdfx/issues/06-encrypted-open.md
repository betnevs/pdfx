# 06: 加密 PDF 打开

**What to build:** 打开带 User Password 的 PDF 时提示输入密码；Owner Password 施加 Permission Restrictions 时进入只读模式并说明原因；密码错误有明确提示。

**Blocked by:** 04 (打开文档并渲染第一页)

**Status:** ready-for-agent

- [ ] 带 User Password 的 PDF 打开时弹出密码输入，正确密码可进入。
- [ ] 密码错误时提示重试，不泄露文档内容。
- [ ] Owner Password 限制编辑时，编辑工具禁用并显示只读原因。
- [ ] 只读 Document 的保存入口禁用或明确提示“另存为不加密副本”的后果。
