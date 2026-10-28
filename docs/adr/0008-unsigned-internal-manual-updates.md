---
Status: superseded by ADR-0009
---

# 内部阶段不签名、手动更新；公开发布前再公证并启用自动更新

内部阶段（M0–M2）的 macOS 与 Windows 构建都不做 OS 代码签名，安装包通过手动分享，更新由用户手动下载新版本；应用在这一阶段完全不联网。公开发布前再购买 Apple Developer Program 与 Windows 代码签名证书，完成公证后启用 Tauri 自动更新。这样避免在验证产品前承担证书年费，也避免未签名自动更新被 Gatekeeper/SmartScreen 拦截；Tauri 的更新签名密钥可以提前生成，但只有签名后的构建才启用更新端点。
