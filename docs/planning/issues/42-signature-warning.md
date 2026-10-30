# 42: 数字签名失效警告

**What to build:** 打开含数字签名的 PDF 时提示“编辑或保存会使签名失效”；保存前再次确认。只提示，不阻止保存；不执行证书签名。

**Blocked by:** 10 (Save As / Save Changes 管线)

**Status:** done

- [x] 打开含签名或签名字段的 PDF 时显示明确警告，不泄露文档内容。
- [x] 保存前确认；用户取消则取消保存，确认则继续。
- [x] 不执行证书签名，不修改签名对象；保存后警告仍说明签名已失效。
- [ ] fixture 覆盖至少一个带签名结构的 PDF；在 Preview、Acrobat Reader 与 Chrome 中打开不崩溃。

**Verification notes:**

合并与修复：merge commit `6e95331`（`ticket/42-signature-warning` 分支头 `020df68`，以 `--no-ff` 合入 `integration/pdfx`；集成分支原 tip `47dffb4` 是其祖先，无文本冲突）。独立审计发现并修复一个真实缺陷：直接列在 `/AcroForm /Fields` 的 widget 若只通过 `/Parent` 继承所属字段的 `/FT` 与 `/V`（所属字段本身不在 `/Fields` 树中），原实现不会跟随根条目的 `/Parent` 链，真实签名会被漏报。修复提交 `45bacfe` 在没有树内祖先时沿 `/Parent` 链读取继承的 `/FT` 与 `/V`（父链最多 32 层），并为 `/Kids` 递归增加 128 层深度上限；回归测试覆盖该形状的检测、保存门禁与结构保真，另覆盖 2 万层深的字段树、`/Kids` 环，以及 AES-256 加密文档使用打开密码的检测。

独立验证的行为：

- 结构与计数：`signature.rs` 用 lopdf 走 `/AcroForm /Fields` 与嵌套 `/Kids`，按 PDF 32000-1 Table 220 继承 `/FT`，只有 `/FT /Sig` 字段的 `/V` 为字典时才计为“已签名”；空签名字段只计入字段数。PDFium 的 `signatures()`（含空字段）作为字段计数的下限，结构无法解析时退回逐页统计签名 widget，因此畸形文件也不会丢掉警告。跨命令边界的 `SignatureSummary` 只有 `signedCount` 与 `fieldCount` 两个计数。
- 安全边界：字段树递归按对象 ID 去重，`/Kids` 环只遍历一次（永久测试断言两个环内字段各计一次）；2 万层深的字段树在打开时被深度上限截断，不耗尽栈；审计探针另验证了 2 万层深、2 万个同级条目与环状 `/Kids` 均不 panic、不挂起。普通文本字段、复选框等表单字段不计入，`signature_image` 套件证明 PDFX 图片签名是页面图片对象，不是数字签名。
- 不泄露内容：`DocumentView` 序列化为 JSON 后不含 signer 名称 canary、签名原因 canary、`ByteRange` 或 `Contents`；拒绝保存与成功保存后的本地日志同样不含这些内容。UI 只显示中文计数文案。
- 保存确认：唯一写入缝 `save_document_with_progress` 在序列化之前检查 `confirm_signature_invalidation`（`#[serde(default)]`，默认 false），未确认返回 `SignatureInvalidationConfirmationRequired`。取消保存不写任何字节：输出文件不存在、无临时/备份文件、源文件字节与 mtime 不变、脏标记与撤销历史保留，可再次保存。确认标志按每次请求独立，从不持久化：连续两次保存都会再次询问；确认后写入失败的重试仍会再问；恢复文件还原出的文档走同一门禁。只读文档的 Save As 仍需确认；加密输出（用户密码）保存后签名结构保留。
- 不修改签名对象：独立用 lopdf 解析保存结果，签名字典与源文件逐项相等，`/ByteRange` 仍为 `[0 1234 5678 9012]`，`/Contents` 仍为原 64 字节，`/SigFlags` 保持 1，字段树名称、类型与 `/Kids` 保持；未引入 `/NeedAppearances`，catalog 未新增 `/Perms`。PDFium 重开并渲染输出无崩溃，源文件从未被改写。全程没有证书签名或按签名值重写对象的代码路径。
- 前端流程：打开含签名结构（已签或仅有字段）的文档时显示可关闭、非阻塞的 `role="status"` 中文横幅，文案区分“已签名”和“仅签名字段”；保存对话框在同一确认流程中组合必填字段与加密决定，取消不调用保存；菜单“保存/另存为”和窗口关闭守卫均经由同一个 `useDocumentSave` 门禁；确认状态不写入设置或文档。成功保存后 `saveGeneration` 触发横幅重新出现，并切换为“签名已失效”文案；`signatureInvalidated` 只在保存结果含实际签名时为 true。
- fixture：`fixtures/manifest.json` 新增 `signature-signed`、`signature-field-only`、`encrypted-signature-restricted` 三个确定性样例，固定 SHA-256 与尺寸，`fixtures/README.md` 有说明；前两个分别覆盖已签名 + `/Parent` 继承空字段、两个空签名字段，后者覆盖带所有者限制的加密签名文档。用 `swift fixtures/verify.swift` 以 macOS PDFKit 独立验证：全部 fixture 的尺寸与 SHA-256 匹配，三个签名样例均可打开，各页 2 个 widget，声明 `/FT /Sig` 字段，仅字段样例不含签名值。测试 canary 只存在于合成 PDF 与 fixture 中，用于证明不泄露。
- 依赖与边界：没有新增依赖（`Cargo.toml`、`Cargo.lock`、`package.json`、`package-lock.json` 未变），vendor 树未改；无遥测、无网络请求、无密钥。

门禁（合并与修复提交之后的主仓库）：`npm run typecheck` 通过；`npm test` 51 个文件 / 622 个测试通过；`npm run build` 通过；`npm run test:e2e` 12 个通过；`cargo fmt --check` 通过；`cargo clippy -- -D warnings` 通过，仅有 4 条 vendored `pdfium-render` 既有 warning；`cargo test` 38 个测试目标，465 通过 / 0 失败 / 3 ignored（`signature_warning` 套件 14 个测试全部通过）；`swift fixtures/verify.swift` 全部 fixture 检查通过。测试支持代码的 `dead_code` warning 是仓库既有的按测试二进制裁剪现象，不参与 clippy 门禁。

限制与待办：

- 第 4 项未勾选：fixture 覆盖已完成，且 PDFKit（Preview 的 PDF 引擎）已实际打开三个签名样例；但本环境没有可人工核对的 Acrobat Reader 与 Chrome PDF 阅读器，这两个阅读器的打开检查仍待执行。PDFium 重开渲染与独立 lopdf 解析已作为替代验证。
