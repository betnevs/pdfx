# 46: 打印与导出的健壮性

**What to build:** 打印大文档时不再把所有页面位图同时解码进内存，导出图片采用原子写入，并且导出目标不能覆盖 Source Document 或当前 Output Document。

**Blocked by:** 27 (打印与导出当前页)

**Status:** ready-for-agent

- [ ] 打印按页或分块准备/解码位图，内存占用有上限；2000 页全范围打印不会一次性解码所有页面。
- [ ] 打印取消会释放已准备的位图与临时状态，不残留 DOM/内存。
- [ ] PNG/JPG 导出先写同目录临时文件再原子重命名；磁盘满或中断不会在目标处留下半张图片。
- [ ] 导出目标与 Source Document、当前 Output Document（含符号链接/大小写变体）相同时拒绝，并显示明确中文原因。
- [ ] 大文档打印与导出有可验证的进度/取消行为；测试覆盖内存上限路径与目标保护。

**Notes:** ticket 27 的审计发现：打印表面把每一页 150 DPI PNG 以 base64 保存在 print DOM，并用 `Promise.all` 同时解码（Letter 每页 RGBA 约 8.4MB，2000 页可能达数十 GB）；导出使用非原子 `fs::write`；导出只保护 Source Document，选当前 Output Document 路径会把已保存的 PDF 替换成图片字节（仅靠系统覆盖确认）。本票只做健壮性修复，不改变 ticket 27 的交互与快捷键；菜单项置灰属 ticket 45。
