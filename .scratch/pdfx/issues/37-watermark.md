# 37: 水印

**What to build:** 用户可以添加文字或图片水印，设置透明度、旋转、位置与缩放，并应用到当前 Page 或全部 Page；保存后水印成为页面内容。

**Blocked by:** 10 (Save As / Save Changes 管线)

**Status:** done

- [x] 文字/图片水印可预览，透明度、旋转、位置、缩放可调。
- [x] 可选择当前 Page 或全部 Page 应用；重复应用不会叠加出意外结果。
- [ ] 保存后水印在其他阅读器中位置与透明度一致。
- [x] 撤销/重做覆盖水印添加与删除。

## Verification notes

Merge commit `e3b89da` (`ticket/37-watermark` merged with `--no-ff` into `integration/pdfx` at `ee25dee`, no conflicts). Audit fixes `6d5566e` (copy tracking and text targeting) and `093b5f0` (thumbnail invalidation) sit on the feature commit `4264be7`; the branch tip before the main merge was `093b5f0`. Close commit: `chore: close ticket 37` (the last change to this file).

Independently verified behavior:

- Mark model: the session keeps at most one active watermark (`WatermarkState`), so apply replaces rather than stacks; an identical re-apply on the same page set is a true no-op (revision unchanged, zero pixel diff, one text occurrence). Text marks are appended `PdfPageTextObject`s using the bundled Noto Sans SC subset (the saved font is the bundled subset, not a system font, and no new asset is added). Image marks are appended `PdfPageImageObject`s; opacity is baked into the BGRA bitmap and persists as a real `/SMask` (source plus mark give two image XObjects, exactly one `/SMask`), and the mark appears in text extraction because it is page content, not an annotation.
- Adjustment and coordinates: `build_text_object` and `image_layout` place marks in display space through `DisplayGeometry`, so CropBox and `/Rotate` are accounted for. All-pages tests assert upright and centered marks on mixed page sizes with /Rotate 0/90/180/270, and image marks keep their aspect ratio. Pixel tests confirm opacity, rotation, position and scale change the rendered mark, and that the saved mark matches the live canvas within a 24-level channel tolerance after PDFium reopen.
- Scope and page targeting: "all pages" targets only visible logical slots (`entry.pages.order`), so deleted/hidden pages are skipped. A scope test hides a page, applies all, undoes both actions in order, and redoes them, asserting the restored hidden page never received a mark and the redone state lists only the two visible slots. Replacing an all-pages mark with a current-page mark removes the old mark from the other pages (scope-narrowing test), and a 500-page all-pages apply is one history action. A copy of a marked page is now tracked as part of the same watermark: replace, delete, undo and redo affect the copy too, and the copy plus its mark survive save and reopen; image copies are covered separately.
- Save fidelity: text and image marks survive save and PDFium reopen; independent lopdf parses show the mark as page content (no annotation), source bytes and mtime are unchanged, repeat saves keep object, font and content counts stable (no duplicate fonts or objects), and the bundled font is a subset. PNG/JPG page export and flatten export include the mark; the print surface renders the same canvases, so the export pixel assertion covers print output. Flatten keeps the mark as page content, not an annotation.
- History: apply, replace and delete are one `EditAction::Watermark` each, with detached object snapshots; tests assert exact page-object restoration after undo/redo, revision counts, dirty/savedRevision, undo after save making the document dirty and redo returning it clean, and the close guard/menu consume the same history. Page operations after a mark (rotate, move, delete) keep targeting correct. The audit found and fixed two defects: copying a marked page left the cloned mark untracked, and watermark text was offered as an editable Text Box. Text Box deletion before the mark shifts its recorded range correctly, so delete still detaches only the mark. After save, replace and delete still remove the live mark and write the correct output.
- Read-only/UI: read-only and restricted documents refuse apply/delete with Chinese messages and no history mutation; the panel exposes text/image modes, opacity, rotation, scale, x/y and scope with Chinese labels, validates empty, multi-line and over-200-character text plus a missing image without invoking the backend, and maps backend errors to Chinese copy. Applying reports whether all thumbnails are stale; the fix makes a replacement that narrows scope invalidate every thumbnail. Undo/redo and page-structure changes refresh the panel and all thumbnails. Watermark actions flow through `save.acceptHistory`, so the dirty marker, native menu and close guard stay consistent.
- Security/integration: images enter only through the native picker and a data-URL preview; the backend reads the chosen path only (missing, unreadable, unsupported, undecodable and oversized files are reported), with the existing 25 MiB / 20 MP limits. There is no network or telemetry, no plaintext or page data in logs, no new dependency, no spec/ADR/packaging/capability change, and the vendor tree is unchanged (15 files, 10 `close(self)` types). Existing annotations, links, form fields and content objects are untouched; the mark is appended page content.

Key tests: `src-tauri/tests/watermark.rs` (26 tests, 7 added by this audit: copy tracking for text and image, text-target refusal, object-range shifts, post-save replace/delete, page operations, scope narrowing), `src/components/WatermarkPanel.test.tsx` (9 tests), plus App, PageViewer and documentEvents wiring tests.

Gate on the merged main repo: `npm run typecheck` passed; `npm test` 47 files / 568 tests passed; `npm run build` passed; `npm run test:e2e` 12 passed; `cargo fmt --check` passed; `cargo clippy -- -D warnings` passed with only the 4 vendored `pdfium-render` warnings; `cargo test` 426 passed / 3 ignored / 0 failed (watermark suite 26).

Limitations / manual items:

- Box 3 (real-reader matrix) is not checked: Preview, Adobe Acrobat Reader and Chrome were not available in this environment. Independent lopdf parses and PDFium pixel reopens confirm the saved structure, position and opacity; a manual check in those readers remains.
- The active watermark is Edit Session state, not document state: after reopening a saved PDF the mark is ordinary page content, so the watermark tool cannot replace or delete it (it reports that there is no watermark). That matches the one-active-watermark-per-session model, and the saved mark itself is fully preserved.
- Copy plus identical re-apply: copying a marked page adds the copy to the active watermark, so re-applying the identical configuration afterwards is no longer a true no-op; it replaces the mark and drops it from the copy. No mark stacks, but the copy loses the mark until it is re-applied.
- Each all-pages apply adds one text or image object per page; a 500-page apply is a single history action and its undo snapshot holds one object per marked page, so memory scales linearly with the marked page count.
- Rotation direction was verified through matrix math and upright rendering tests, not a per-glyph orientation assertion; visual orientation remains covered by the manual reader check.
- The pre-existing 10 `dead_code` warnings in test-support code and the 4 vendored `pdfium-render` warnings remain; the gate's `-D warnings` applies to the app crate only.
