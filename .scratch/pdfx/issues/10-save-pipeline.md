# 10: Save As / Save Changes 管线

**What to build:** 首次保存走 Save As，之后 Save Changes 覆盖当前 Output Document；Source Document 永不修改。保存使用分阶段增量保存，保留书签、链接、元数据、已有 Form Field 与未改动页面内容；覆盖前自动备份，失败时源文件与已有输出保持完整。

**Blocked by:** 08 (编辑已有文字 + 撤销/重做), 09 (新增 Text Box 与样式), 41 (PDFium 增量保存与注释底层适配)

**Status:** ready-for-agent

- [ ] Save As 选择目标路径并写出 Output Document；Save Changes 覆盖当前 Output Document。
- [ ] Source Document 的修改时间与内容不变；覆盖 Output Document 前生成备份。
- [ ] 保存后书签、链接、元数据、已有 Form Field 与未编辑页面内容仍存在。
- [ ] 保存失败（磁盘满、权限、路径占用）时源文件与已有输出完整，显示可重试错误。
- [ ] 加密 Source Document 保存时提示“输出将不加密”，用户确认后才继续。
- [ ] 大文件保存显示进度，保存期间 UI 不冻结；保存结果在 Preview、Adobe Acrobat Reader、Chrome、Edge 中可打开。
