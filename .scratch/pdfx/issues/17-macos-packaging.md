# 17: macOS 打包

**What to build:** 产出 macOS 12+ 的 universal 构建与 `.dmg`，包含未签名安装说明；Apple Silicon 与 Intel 都能启动。

**Blocked by:** 04 (打开文档并渲染第一页)

**Status:** ready-for-agent

- [ ] universal `.dmg` 在 Apple Silicon 与 Intel macOS 12+ 上可安装并启动。
- [ ] 首次启动的 Gatekeeper 绕过说明（系统设置“仍要打开”或 `xattr` 命令）随包提供。
- [ ] 应用名称、图标、版本号与 PDFX 品牌一致；文件关联与 Info.plist 配置正确。
- [ ] 不包含 Apple 公证或代码签名；文档明确说明这是零成本分发的取舍。
