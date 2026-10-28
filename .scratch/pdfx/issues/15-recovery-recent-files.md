# 15: 恢复文件与最近文件

**What to build:** 编辑过程中自动写入 Recovery File；异常退出后下次启动提示恢复；最近文件列表可重新打开 Document。

**Blocked by:** 10 (Save As / Save Changes 管线)

**Status:** ready-for-agent

- [ ] 编辑动作节流写入 Recovery File，Recovery File 不覆盖 Source Document 或 Output Document。
- [ ] 模拟异常退出后重启，出现恢复提示并能找回未保存修改。
- [ ] 正常关闭并保存后，对应 Recovery File 清理。
- [ ] 最近文件列表本地持久化，点击可重新打开；文件不存在时提示并移除。
- [ ] 正常启动不自动打开文档。
