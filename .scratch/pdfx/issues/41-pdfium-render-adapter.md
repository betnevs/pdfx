# 41: PDFium 增量保存与注释底层适配

**What to build:** 一个生产可用的 PDFium 适配层，补齐 `pdfium-render` 0.9.4 缺失的能力：带 flags 的保存（`FPDF_INCREMENTAL` / `FPDF_REMOVE_SECURITY`）、圆形注释创建、墨迹笔画、边框宽度与自定义 `/AP`。实现可以是最小 vendored patch，也可以是受控 raw FFI；必须隔离在适配层内，升级 PDFium 时只需改这一处。

**Blocked by:** 03 (M0→M1 技术验证), 04 (打开文档并渲染第一页)

**Status:** done

- [x] 保存入口可显式选择增量保存与全量重写；增量保存的输出以原始字节为前缀，并保留书签、链接、元数据与未编辑页面。
- [x] 圆形注释、墨迹笔画、边框宽度与自定义 `/AP` 通过适配层可用；保存并重新打开后可见。
- [x] 适配层不把 PDFium 类型泄露到命令边界，调用方不需要 `unsafe`。
- [x] 在 pinned PDFium 151.0.7881.0 上通过集成测试；若采用 vendored patch，记录补丁范围、校验方式与升级步骤。
- [x] 加密源的全量去密重写路径有测试或明确标注为后续票的阻塞项，不静默产生加密输出。

**Verification notes:**

- 合并提交 `2583ce5`（ticket/41-pdfium-render-adapter `18807fe`），无冲突。合并内容：`src-tauri/src/pdfium_adapter.rs`、`src-tauri/tests/pdfium_adapter.rs`、`src-tauri/vendor/pdfium-render/`（vendored crate + `PDFX-PATCH.md`）、`Cargo.toml` 的 `[patch.crates-io]`、`Cargo.lock`、`.gitignore`、`lib.rs`。
- Vendored patch 独立核对：与 crates.io `pdfium-render-0.9.4`（本地 registry 解包）逐文件 `diff -rq`，仅 `document.rs`、`page/annotations.rs`、`page/annotation.rs`、`page/annotation/circle.rs`、`page/annotation/ink.rs` 五个文件有差异，其余完全一致；有意省略 `include/`、`test/` 与打包元数据（动态链接不需要），`LICENSE.md` 保留，`.crate` sha256 `8948a803…5a19` 记录在 `PDFX-PATCH.md`。仓库未提交任何二进制：`.gitignore` 只放行 crate 源码，继续忽略 `src-tauri/vendor/downloads/` 与 `src-tauri/vendor/pdfium/`。
- 自动验证通过：`npm ci`、`npm run setup:native`（确认 PDFium `151.0.7881.0`）、`npm run typecheck`、`npm test`（29 项）、`npm run build`、`npm run test:e2e`（2 项）、`cargo test`（25 项通过、1 项默认忽略；其中 `tests/pdfium_adapter.rs` 6 项全部通过）、`cargo clippy -- -D warnings`（exit 0）。
- 勾选 1：`SaveMode::{Incremental, FullRewrite, RemoveSecurity}` 映射到 `FPDF_INCREMENTAL=1` / `FPDF_NO_INCREMENTAL=2` / `FPDF_REMOVE_SECURITY=4`，经 patched `save_to_bytes_with_flags()` → `FPDF_SaveAsCopy()` 在同一个 `PdfDocument` 句柄上保存（无“另开 raw 文档重写”的 spike 绕路）。测试断言增量输出以源字节为前缀，且重开后可读书签、URI 链接、Title 元数据、附件、文本层与同一文档上的高层 square 注释；全量重写测试断言输出不是前缀且同样保留上述结构。未编辑页面的保真由 500 页 fixture 的全量重写测试以“页面 1 渲染字节前后完全一致”覆盖；增量路径的未编辑页面由“源字节前缀”保证（单页 structure fixture 未逐页对比）。
- 勾选 2：circle 测试创建圆、设边框宽度 4.0、装自定义 `/AP`，保存重开后 `FPDFAnnot_GetBorder` 回读 4.0，渲染新增 >30k 非白像素、>20k 蓝色像素；ink 测试两条笔画经 `FPDFAnnot_AddInkStroke` 写入、重开后 `FPDFAnnot_GetInkListCount` 为 2，自定义 `/AP` 同样渲染可见。创建返回的具体注释类型没有 `Drop`，patch 为 circle/ink 增加 `close(self)`（`FPDFPage_CloseAnnot`），适配层创建后立即关闭；通过枚举取得的注释由上游 `Drop` 关闭。
- 勾选 3：适配层公开类型只有 `AnnotationRect`、`AnnotationRef`、`SaveMode`、`AdapterError`，不暴露任何 `FPDF_*` 句柄，patch 也未增加 raw handle 访问器；`src-tauri/src/` 中除 vendored crate 外没有 `unsafe`，测试调用方同样不需要 `unsafe`。说明：适配层尚未接入 Tauri command（保存管线是 ticket 10）；`new()/document()/into_document()` 使用 pdfium-render 的安全 `PdfDocument`（命令层自 ticket 04 起已持有该类型），接入时需把 `AdapterError` 映射为 `CommandErrorCode`。
- 勾选 4：`scripts/setup-native.sh` 固定 `PDFIUM_VERSION="151.0.7881.0"`（`chromium/7881`），上述 6 项集成测试在该库上通过；补丁范围、校验和、验证命令与五步升级流程记录在 `src-tauri/vendor/pdfium-render/PDFX-PATCH.md`。
- 勾选 5：两个加密 fixture 均覆盖——非空用户口令（`encrypted-user-password`）与空用户口令/仅 owner 限制（`encrypted-owner-restrictions`）。两者都报告 `security_handler_revision != Unprotected`；增量与普通全量重写均返回 `AdapterError::EncryptedDocument`，不会静默写出加密字节；`RemoveSecurity` 输出不以源字节开头、无口令可重开为 `Unprotected` 且页数不变。

**Known limitations / follow-ups:**

- `AnnotationRef` 是 `(page_index, 注释列表下标)`，目前没有删除/重排注释的入口，因而未触发下标失效；后续注释编辑票必须重新解析引用或重编号，不能长期缓存下标。
- 拖动注释时的帧率/时延未在本票测量；适配层不增加每帧保存开销，但交互性能留给视口/渲染票验证。
- ticket 10 的保存管线必须把 PDFium 增量输出作为字节基底、在其上追加 lopdf revision，不能重新打开第二个文档（ADR-0010；spike AC2 的绕路不得上线）。
- 外观设置顺序固定为 `set_bounds` → `set_border_width` → `set_appearance`：PDFium 设置边框会丢弃已有 `/AP`，patch 与适配层文档均已注明；空笔画列表会被 PDFium 拒绝并映射为 `AdapterError`（未单测）。
- patch 只给 circle/ink 具体类型加了 `close(self)`；其他具体注释类型仍无 `Drop`，若将来直接创建需照此扩展（`PDFX-PATCH.md` 已记录）。
- 既有问题（与本 ticket 无关，合并前即存在）：`cargo fmt --check` 仍对 `src-tauri/src/menu.rs` 与 `src-tauri/tests/fixtures.rs` 报格式差异；`cargo clippy --all-targets -- -D warnings` 因 `src-tauri/tests/support/fixtures.rs` 的 dead_code 失败（新测试文件复用该 support，故同样受影响）。相关文件自 `9e3f338` 起未改动，建议另开维护 ticket；本票门禁使用不带 `--all-targets` 的 clippy。
