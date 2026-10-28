# PDFX fixture corpus

固定 PDF 测试语料，供 Rust 集成测试与后续命令边界测试复用。文件全部由 `generate.py` 以纯 Python 标准库生成，确定性输出（固定文档 ID，无时间戳），不依赖 qpdf、PDFium 或 lopdf。

## 目录

```
fixtures/
  manifest.json     # 机器可读索引：类别、sha256、页数、密码、预期有效性
  generate.py       # 唯一的生成入口
  verify.swift      # 用 macOS PDFKit 独立验证生成结果
  pdfs/             # 生成产物（已提交）
```

Rust 测试通过 `src-tauri/tests/support/fixtures.rs` 读取 `manifest.json`，用 `fixtures::fixture(&manifest, "encrypted")` 按类别取样例。

## 语料清单

| 类别 | 文件 | 说明 | 密码 |
| --- | --- | --- | --- |
| text | `text-latin.pdf` | Helvetica 单页拉丁文本，文字可提取 | — |
| scanned | `scanned-image-only.pdf` | 纯 DeviceGray 图像页，无文本层 | — |
| encrypted | `encrypted-user-password.pdf` | 必须输入用户密码才能打开 | 用户 `pdfx-user` / 所有者 `pdfx-owner` |
| encrypted | `encrypted-owner-restrictions.pdf` | 用户密码为空；所有者限制修改、复制与注释，允许打印 | 用户（空）/ 所有者 `pdfx-owner` |
| encrypted | `encrypted-print-denied.pdf` | 用户密码为空；所有者仅禁止打印，编辑、复制与注释仍允许 | 用户（空）/ 所有者 `pdfx-owner` |
| form | `form-acroform.pdf` | 已填写的文本字段 + 复选框（`/Off`、`/Yes` 外观流） | — |
| structure | `structure-metadata.pdf` | 两级书签、URI 链接、Info/XMP 元数据、内嵌附件 | — |
| large | `large-500-pages.pdf` | 500 页最小“大文件”样例（约 146KB） | — |
| rotated | `rotated-mixed-size.pdf` | 旋转 90/0/270，三种页面尺寸 | — |
| malformed | `malformed-truncated.pdf` | 文件中途截断，缺少 xref/trailer；PDFKit 拒绝打开 | — |
| malformed | `malformed-bad-xref.pdf` | `startxref` 指向不存在的偏移；PDFKit 扫描修复后仍可打开（畸形但可恢复） | — |

加密样例使用 PDF 1.4 Standard Security Handler（V1/R2，40-bit RC4）。`manifest.json` 中 `valid: false` 只表示“故意畸形”，不代表文件为空或不可读。

## 重新生成与检查

```bash
python3 fixtures/generate.py          # 写入 pdfs/ 与 manifest.json
python3 fixtures/generate.py --check  # 不写文件；与提交内容不一致时退出 1
swift fixtures/verify.swift           # 用 PDFKit 独立断言每个样例的行为
```

`--large-pages N` 可生成不同页数的替代大文件（例如 `--large-pages 2000` 做压力样例）。默认 500 页是刻意缩小的提交版本，避免仓库膨胀。

PDFKit 验证脚本覆盖：文本可提取、扫描件无文本层、加密文件锁定/解锁与权限位、表单 Widget、书签/链接/附件/XMP、页数、旋转、页面尺寸、截断文件不可打开。生成器与验证器使用不同的 PDF 实现，避免“自己验证自己”。

## 暂未包含（后续票）

- 中文/混排、Embedded Font、Font Subset、CID/Type0 字体样例：需要引入许可明确的字体文件，属于字体管线票的范围。
- 各类 Annotation、AcroForm 创建结果、数字签名、透明与图片混合：随对应功能票补充。

新增类别时，先扩展 `generate.py` 的 `build_all()` 与 `manifest.json` 条目，再在 `verify.swift` 中加入独立断言，最后同步 `src-tauri/tests/fixtures.rs` 的必需类别列表。
