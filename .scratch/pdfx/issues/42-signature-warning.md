# 42: 数字签名失效警告

**What to build:** 打开含数字签名的 PDF 时提示“编辑或保存会使签名失效”；保存前再次确认。只提示，不阻止保存；不执行证书签名。

**Blocked by:** 10 (Save As / Save Changes 管线)

**Status:** ready-for-agent

- [ ] 打开含签名或签名字段的 PDF 时显示明确警告，不泄露文档内容。
- [ ] 保存前确认；用户取消则取消保存，确认则继续。
- [ ] 不执行证书签名，不修改签名对象；保存后警告仍说明签名已失效。
- [ ] fixture 覆盖至少一个带签名结构的 PDF；在 Preview、Acrobat Reader 与 Chrome 中打开不崩溃。
