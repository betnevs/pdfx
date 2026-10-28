# 在 macOS 上安装 PDFX

PDFX 以**未签名、未公证**的 universal `.dmg` 分发，同时支持 Apple Silicon 与 Intel，最低要求 macOS 12。因为没有 Apple Developer Program 证书，macOS 的 Gatekeeper 会在首次启动时阻止应用；这是零成本分发的预期取舍，不是文件损坏。

## 安装

1. 打开下载的 `PDFX_0.1.0_universal.dmg`。
2. 把 `PDFX` 拖进“应用程序”文件夹。
3. 从“应用程序”启动 `PDFX`。首次启动会被 macOS 阻止，任选一种方式放行：
   - **macOS 12–14**：在“应用程序”里按住 Control 点击（或右键点击）`PDFX`，选择“打开”，在确认框中再点“打开”。
   - **macOS 13 及更高版本**：先双击 `PDFX`，被阻止后打开 **系统设置 → 隐私与安全性**，在“安全性”区域找到被拦截的 PDFX，点击 **“仍要打开”**，输入登录密码并确认。（macOS 15 起 Apple 移除了 Control 点击放行的快捷方式，请使用此路径。）
   - **终端**（三种系统版本都适用）：

     ```bash
     xattr -dr com.apple.quarantine /Applications/PDFX.app
     ```

     然后重新启动 PDFX。

放行成功后，macOS 会记住该应用，后续启动不再拦截。若把应用移动到其他位置，需要对新位置重新执行一次 `xattr` 命令。

## 为什么会出现这个提示

- 构建产物没有 Apple Developer ID 签名，也没有经过 Apple 公证。`spctl` 会拒绝它，这是预期行为。
- Apple Silicon 要求可执行文件至少有 ad-hoc 签名；构建时由链接器自动完成，PDFium 动态库也带有 ad-hoc 签名。请勿用 `codesign --remove-signature` 或修改 `Contents/Frameworks/libpdfium.dylib`，否则应用无法加载 PDF 引擎。
- 本项目不购买付费证书，以换取零分发成本；详见 `docs/adr/0009-zero-cost-distribution.md`。
- 应用不包含自动更新：新版本需要手动下载并替换。启动时不会联网检查更新。

## 应用内文件

- `Contents/Frameworks/libpdfium.dylib`：PDFium 渲染引擎（universal）。
- `Contents/Resources/pdfium-licenses/`：PDFium 的 BSD-3-Clause 许可证及第三方许可证。
- 本说明也随应用位于 `Contents/Resources/INSTALL-macos.md`。

## 校验下载（可选）

每个发布页应同时给出 `.dmg` 的 SHA-256。下载后可在终端执行：

```bash
shasum -a 256 ~/Downloads/PDFX_0.1.0_universal.dmg
```

将输出与发布页提供的值比对。校验和一致仍不代表 Apple 认可该应用，Gatekeeper 放行步骤不能省略。
