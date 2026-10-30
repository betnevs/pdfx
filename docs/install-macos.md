# 在 macOS 上安装 PDFX

PDFX 以**未签名、未公证**的 `.dmg` 分发，最低要求 macOS 12。提供两种构建：

| 构建 | 适用机器 | 包内代码 |
| --- | --- | --- |
| `PDFX_0.1.0_arm64.dmg` | Apple Silicon（M 系列） | 仅 arm64，推荐 |
| `PDFX_0.1.0_universal.dmg` | Apple Silicon 与 Intel | arm64 + x86_64 |

因为没有 Apple Developer Program 证书，macOS 的 Gatekeeper 会在首次启动时阻止应用；这是零成本分发的预期取舍，不是文件损坏。

## 选择哪个构建

- **Apple Silicon Mac**：优先使用 `PDFX_0.1.0_arm64.dmg`。它不包含任何 Intel 代码，macOS 26.4 及更高版本不会把 PDFX 报告为依赖 Intel 组件的应用。
- **Intel Mac**：使用 `PDFX_0.1.0_universal.dmg`。macOS 27 是 Apple 最后一个支持 Intel Mac 的版本，之后的系统升级需要换用 Apple Silicon 机器。

## 关于 macOS 26.4+ 的“Intel 芯片 App”通知

macOS 26.4 起，系统会在检测到应用包含 Intel 组件时提示“对 Intel 芯片 App 的支持即将结束”。universal 构建为了兼容 Intel Mac，刻意保留了 x86_64 切片，因此在 Apple Silicon 上也可能触发该通知；运行时进程仍然是原生 arm64，并不依赖 Rosetta。这是 Apple 的弃用提示，不是 PDFX 崩溃或安装损坏。

彻底避免该通知请使用 arm64 构建。替换应用后，已经出现在通知中心的旧提示不会自动消失，手动清除即可。

## 安装

1. 打开下载的 `.dmg`（Apple Silicon 推荐 `PDFX_0.1.0_arm64.dmg`，Intel 使用 `PDFX_0.1.0_universal.dmg`）。
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
- Apple Silicon 要求可执行文件至少有 ad-hoc 签名。构建时 Tauri 会用 ad-hoc 身份重新签名整个 `.app`（包括 PDFium 动态库），arm64 构建在裁剪 PDFium 后还会再签一次。请勿用 `codesign --remove-signature` 或修改 `Contents/Frameworks/libpdfium.dylib`，否则应用无法加载 PDF 引擎。
- 本项目不购买付费证书，以换取零分发成本；详见 `docs/adr/0009-zero-cost-distribution.md`。
- 应用不包含自动更新：新版本需要手动下载并替换。启动时不会联网检查更新。

## 应用内文件

- `Contents/Frameworks/libpdfium.dylib`：PDFium 渲染引擎。arm64 构建中只有 arm64 切片，universal 构建中同时包含 arm64 与 x86_64。
- `Contents/Resources/pdfium-licenses/`：PDFium 的 BSD-3-Clause 许可证及第三方许可证。
- `Contents/Resources/font-licenses/`：八款内置字体的 SIL OFL 1.1 许可证与出处清单（含 SHA-256）。
- PDFX 会自动读取本机已安装的字体，包括系统字体与 WPS Office 随版本提供的宋体、仿宋等，出现在“WPS 字体”与“本机字体”分组中；这些字体不随安装包分发，WPS Office 的专属字体只在安装了它的机器上出现。
- 本说明也随应用位于 `Contents/Resources/INSTALL-macos.md`。

## 校验下载（可选）

每个发布页应同时给出 `.dmg` 的 SHA-256。下载后可在终端执行：

```bash
shasum -a 256 ~/Downloads/PDFX_0.1.0_arm64.dmg
# 或
shasum -a 256 ~/Downloads/PDFX_0.1.0_universal.dmg
```

将输出与发布页提供的值比对。校验和一致仍不代表 Apple 认可该应用，Gatekeeper 放行步骤不能省略。
