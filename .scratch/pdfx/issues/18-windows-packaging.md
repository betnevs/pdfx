# 18: Windows 打包

**What to build:** 产出 Windows 10 1809+/11 x64 的 `.msi`、`.exe` 安装器与 portable zip，内置 WebView2 Evergreen Bootstrapper；提供 SmartScreen 绕过说明。

**Blocked by:** 04 (打开文档并渲染第一页)

**Status:** ready-for-agent

- [ ] `.msi`、`.exe` 与 portable zip 在 Windows 10 1809+/11 x64 上可安装/启动。
- [ ] 离线环境下 WebView2 Bootstrapper 能完成安装；不依赖联网的 0MB bootstrapper。
- [ ] SmartScreen 的“更多信息 → 仍要运行”说明随包提供。
- [ ] 应用名称、图标、版本号与 PDFX 品牌一致；文件关联配置正确。
- [ ] 不包含 Windows 代码签名；文档明确说明这是零成本分发的取舍。
