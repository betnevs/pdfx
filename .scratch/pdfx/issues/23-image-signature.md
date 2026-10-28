# 23: 图像签名

**What to build:** 用户可以绘制、输入或上传签名图片，把签名放到 Page 上并调整大小/位置，保存后签名成为页面内容。

**Blocked by:** 14 (插入图片)

**Status:** done

- [x] 支持绘制签名、输入姓名生成签名图、上传 PNG/JPG 三种入口。
- [x] 签名可放置、移动、缩放、旋转，背景透明处理合理。
- [ ] 保存后签名在 Preview、Chrome 中位置与外观一致。
- [x] 撤销/重做覆盖签名插入与变换。

## Verification notes

Merge commit `84fc2a1`（`ticket/23-image-signature` 以 `--no-ff` 合入 `integration/pdfx`，分支基于 `aa64e7f`，无文本冲突；合并后 diff 与功能提交 `3bfe1dd` 完全一致，没有丢失其他分支的工作）。独立审计未发现需要修复的缺陷，因此没有额外的 `fix:` 提交。关闭提交：`chore: close ticket 23`（本文件的最后一次修改）。

独立验证的行为：

- 三种入口：`SignatureDialog` 提供“绘制 / 输入 / 上传”三个页签。绘制画布为 1600×600 透明画布，只描边不填背景，导出时按笔迹包围盒裁剪为透明 PNG；输入页签用内置 Ma Shan Zheng 手写字体把姓名栅格化为透明 PNG（字形变成像素，PDF 不嵌入字体）；上传页签按魔数字节接受 PNG 与 JPEG（`accept="image/png,image/jpeg"`），解码为 RGBA 后按需自动移除白色背景、裁剪透明边距，再统一编码为 PNG。三种入口最终都以内存中的 base64 PNG 交给命令，不落盘。
- 字体资源与离线：`src/assets/fonts/MaShanZheng-Regular.ttf`（5,857,936 字节）已提交，SHA-256 `6d2546bb…88b7fb` 与 `SOURCE.txt` 一致；`name` 表家族名为 “Ma Shan Zheng”、版本 2.003，文件为合法 TrueType；同目录 `MaShanZheng-OFL.txt` 为 SIL Open Font License 1.1 全文，字体 `name` 表也带有 OFL 声明，属可自由嵌入/分发的许可。`npm run build` 产出 `dist/assets/MaShanZheng-Regular-wERnqapb.ttf`，样式表用本地 `@font-face` 引用。用真实 Chromium 打开应用验证：`document.fonts.load("64px 'Ma Shan Zheng'")` 命中字体、`document.fonts.check(...)` 为 true，字体请求来自 `127.0.0.1` 本地开发服务器，整页没有任何外部 HTTP(S) 请求。
- 对象模型：签名是普通图片页面对象，不是 Annotation，也不是数字签名字段。独立用 lopdf 解析保存结果：页面字典没有 `/Annots`，全文件没有 `/Sig`；页面资源只有一个 `/Subtype /Image` 的 DeviceRGB 彩色对象（带 `/SMask` 指向同尺寸 DeviceGray 掩码），内容流为 `q /FXE1 gs ... cm /FXX1 Do Q`。PNG 与 JPEG 上传最终都以页面 XObject 写入，保存走既有图片保存路径。
- 放置与变换：默认矩形按签名用途缩小为页面显示宽度的 35%、高度的 25%（等比、居中，且不放大），避免签名占满整页；`/Rotate` 90 度的页面上按显示几何计算（600×400 显示空间），宽高与中心点由测试断言。移动、缩放、旋转、删除全部复用既有图片覆盖层命令；插入、变换、删除进入同一 Edit Session 历史，撤销/重做逐级恢复位置、尺寸、旋转与 kind。
- 保存保真：透明像素在保存后仍透明——在蓝色底图上插入透明签名，保存并用 PDFium 重开后，红色笔迹不透明、其余区域透出蓝色；重复保存对象数与页面内容不变，不产生重复彩色图片对象（一张彩色图 + 一张掩码）。源文件字节与 mtime 均未变化。PNG/JPG 页面导出与 flatten 导出都包含签名（flatten 计数为 0，因为签名本就是页面内容），重开后同一位置渲染出签名。
- 只读与校验：只读文档在命令边界由 `ensure_content_editable` 拒绝插入/变换/删除，返回 `permission_denied`，图片列表与历史均不变化；前端按钮禁用并显示中文原因。载荷校验在打开文档之前完成：空载荷、非 base64、非 PNG/JPEG 魔数、截断 PNG、截断 JPEG、超过 25 MiB 的 base64（先按长度拒绝再解码）、IHDR 声明超过 2000 万像素、空/超 80 字符/含控制字符的签名名，全部被拒绝；测试确认文档像素、图片列表与撤销/重做状态完全不变。
- 安全与集成：新增代码无遥测、无网络请求、无密钥或敏感数据；日志不记录载荷。没有新增依赖或清单/配置变更（`base64` 在此前已是依赖），`Cargo.toml`、`package.json`、Tauri 配置均未改动。界面文案为简体中文，插入期间禁用页签、输入、取消与提交并显示“正在插入…”，失败时以 `role="alert"` 显示对应中文错误并允许重试。

门禁（合并后的主仓库）：`npm run typecheck` 通过；`npm test` 50 个文件 / 605 个测试通过（新增 3 个文件、32 个测试）；`npm run build` 通过且字体进入产物；`npm run test:e2e` 12 个通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` 通过，仅有 4 条 vendored `pdfium-render` 既有 warning；`cargo test` 37 个测试目标、451 通过 / 0 失败 / 3 ignored（新增 `signature_image` 套件 8 个测试）。`tests/form_creation.rs:1560` 的 `unused variable: updated` 与测试支持代码的 `dead_code` warning 在基线 `aa64e7f` 已存在，不是本合并引入。

限制与待办：

- 第 3 项（Preview/Chrome 真实阅读器外观一致性）未勾选：本环境没有可用的 Preview/Chrome PDF 阅读器人工核对。已用 PDFium 重开渲染像素、独立 lopdf 解析页面结构与变换矩阵作为替代验证；手动阅读器核对仍待执行。
- 合入门禁曾出现一次与本票无关的既有前端测试超时（`TextBoxEditing.test.tsx` 期望 5 次 document-changed 事件、收到 4 次）：当时 Playwright、构建与单测并行运行导致负载抖动；该文件单独运行与整套 `npm test` 串行重跑均通过，与签名代码无调用关系。
- 安装包内的字体许可文件打包是仓库级既有做法（Noto 字体同样只在源码树保留 OFL 文本，构建脚本只复制 PDFium 许可），本票未改变打包流程。
