# 40: 输出密码

**What to build:** 用户可以在 M3 保存时选择给 Output Document 设置打开密码；不设置时保持 ADR-0007 的默认（加密源保存为不加密输出并提示）。

**Blocked by:** 10 (Save As / Save Changes 管线)

**Status:** ready-for-agent

- [ ] 保存对话框中可启用“设置打开密码”，输入并确认密码。
- [ ] 设置密码后输出文件需要密码才能打开；密码错误无法打开。
- [ ] 不设置密码时，加密源保存仍提示“输出将不加密”。
- [ ] 密码不写入日志、Recovery File 或诊断包。
- [ ] 保存结果在 Preview、Adobe Acrobat Reader、Chrome 中按密码行为验证。
