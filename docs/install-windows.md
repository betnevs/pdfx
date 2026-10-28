# 在 Windows 上安装 PDFX

PDFX 以**未签名**的 Windows x64 构建分发，支持 Windows 10 1809 及以上与 Windows 11 x64。项目没有购买 Windows 代码签名证书，因此 SmartScreen 可能在首次安装或启动时拦截应用。这是零成本分发的预期取舍（见 [`docs/adr/0009-zero-cost-distribution.md`](../docs/adr/0009-zero-cost-distribution.md)），不是文件损坏。

应用名称、图标与版本号均来自 `src-tauri/tauri.conf.json`：`PDFX`、`icons/icon.ico`、当前版本 `0.1.0`。安装版会注册 `.pdf` 文件关联。

## 选择安装包

| 文件 | 用途 |
| --- | --- |
| `PDFX_0.1.0_x64_en-US.msi` | MSI 安装器，适合标准部署与静默安装 |
| `PDFX_0.1.0_x64-setup.exe` | NSIS 安装器，交互最简单 |
| `PDFX_0.1.0_x64-portable.zip` | 免安装便携版；不注册文件关联，需要系统已有 WebView2 |

两个安装器都内置 WebView2 Evergreen Standalone Installer（约 127 MB），因此**离线**环境也能完成 WebView2 安装；不依赖需要联网下载运行时的 0 MB bootstrapper。便携版不内置 WebView2，见下文。

## 安装（MSI / NSIS）

1. 下载 `.msi` 或 `-setup.exe`。如果文件来自浏览器或邮件，先解除 Windows 的“来自其他计算机”标记：右键文件 → **属性** → 勾选 **解除锁定** → **确定**；也可在 PowerShell 中执行 `Unblock-File .\PDFX_0.1.0_x64-setup.exe`。
2. 运行安装器。若出现 SmartScreen 的“Windows 已保护你的电脑”提示：
   - 点击 **更多信息**；
   - 再点击 **仍要运行**。
   - 如果只看到“不运行”，请先确认文件属性中的“解除锁定”已完成。
3. 按向导完成安装。安装器会把 PDFX 注册为 `.pdf` 的打开方式；Windows 可能保留已有的默认应用，可在 **设置 → 应用 → 默认应用** 中把 PDFX 设为默认，或右键 PDF 文件选择 **打开方式 → 选择其他应用**。
4. 从开始菜单启动 PDFX。如果首次启动再次被 SmartScreen 拦截，同样选择 **更多信息 → 仍要运行**。

静默安装（供部署脚本参考，仍需自行评估未签名二进制的风险）：

```powershell
# MSI
msiexec /i .\PDFX_0.1.0_x64_en-US.msi /qn
# NSIS
.\PDFX_0.1.0_x64-setup.exe /S
```

卸载：**设置 → 应用 → 已安装的应用 → PDFX → 卸载**。

## 便携版（portable zip）

1. 建议在解压**之前**右键 zip → **属性** → **解除锁定**，然后解压到任意目录。
2. 若用命令行解压，解压后请用 PowerShell 解锁可执行文件与 DLL：

   ```powershell
   Unblock-File .\PDFX.exe
   Unblock-File .\pdfium.dll
   ```

3. 双击 `PDFX.exe` 启动。便携版不写注册表、不注册文件关联；需要文件关联时请使用 MSI/NSIS 安装版，或手动通过 **打开方式** 选择 `PDFX.exe`。
4. 便携版要求系统已安装 WebView2 Runtime。Windows 11 与仍在接收 Edge 更新的 Windows 10 通常已自带；如果启动失败或提示缺少 WebView2，请改用安装版（会离线安装 WebView2），或从 Microsoft 下载 Evergreen Standalone Installer 手动安装。可用以下命令检查：

   ```powershell
   $paths = @(
     'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
     'HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
   )
   $paths | ForEach-Object { Get-ItemProperty -Path $_ -ErrorAction SilentlyContinue }
   ```

   输出中包含 WebView2 版本即表示已安装。

## 为什么会出现 SmartScreen 提示

- 构建产物没有 Authenticode 代码签名，也没有时间戳。SmartScreen 对没有信誉的新发布者会拦截下载或运行，点击 **更多信息 → 仍要运行** 即可放行。
- 项目不购买代码签名证书，以换取零分发成本；详见 [`docs/adr/0009-zero-cost-distribution.md`](../docs/adr/0009-zero-cost-distribution.md)。
- 应用不包含自动更新，也不会在启动时联网检查更新；新版本需要手动下载并安装。应用不含遥测。

## 校验下载（可选）

发布页同时提供 `SHA256SUMS-windows.txt`，其中列出三个产物的 SHA-256。下载后在 PowerShell 中执行：

```powershell
Get-FileHash -Algorithm SHA256 .\PDFX_0.1.0_x64-setup.exe
```

把输出与 `SHA256SUMS-windows.txt` 中对应行比对。校验一致只说明文件完整，不代表文件已签名。

## 构建

需要 Windows 10 1809+/11 x64、Node.js `^22.13.0` 或 `>=24`、Rust 的 `x86_64-pc-windows-msvc` 目标、MSVC 生成工具，以及 Git for Windows（提供 `bash`，供 `scripts/setup-native.sh` 使用）。在仓库根目录运行：

```powershell
rustup target add x86_64-pc-windows-msvc   # 仅首次
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
```

脚本会依次执行 `npm run setup:native`、`npx tauri build --target x86_64-pc-windows-msvc --bundles msi,nsis --ci --no-sign`、组装 portable zip，并输出：

```text
src-tauri\target\x86_64-pc-windows-msvc\release\bundle\msi\PDFX_0.1.0_x64_en-US.msi
src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis\PDFX_0.1.0_x64-setup.exe
src-tauri\target\x86_64-pc-windows-msvc\release\bundle\portable\PDFX_0.1.0_x64-portable.zip
src-tauri\target\x86_64-pc-windows-msvc\release\bundle\SHA256SUMS-windows.txt
```

首次打包时构建机需要联网下载 WebView2 离线安装器；生成后的 MSI/NSIS 安装器可在无网络的机器上完成安装。重复构建可加 `-SkipNativeSetup` 复用已下载的 PDFium。

在没有 Windows 的机器上可运行静态配置检查：

```bash
node scripts/verify-windows-config.mjs
```

该检查只验证配置、资源映射、脚本与文档的一致性，不能代替真机验证。发布前请在干净的 Windows 10 1809 与 Windows 11 x64 虚拟机上各测一次：MSI/NSIS 安装与卸载、启动、打开 PDF、文件关联，以及断网后安装 WebView2。

## 随包文件

- 安装目录中的 `pdfium.dll`：PDFium 渲染引擎（x64）。
- 安装目录中的 `pdfium-licenses/`：PDFium 的 BSD-3-Clause 许可证及第三方许可证。
- 安装目录中的 `INSTALL-windows.md`：本说明的副本。
- 便携包根目录：`PDFX.exe`、`pdfium.dll`、`pdfium-licenses/`、`INSTALL-windows.md`。
