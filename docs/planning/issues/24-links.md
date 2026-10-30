# 24: 超链接

**What to build:** 用户可以把文字或图片链接到 URL 或文档内 Page，编辑或删除已有链接，保存后在阅读器中可点击。

**Blocked by:** 07 (文本提取与文本层), 14 (插入图片), 10 (Save As / Save Changes 管线)

**Status:** done

- [x] 选中文字或图片后可添加 URL/Page 目标链接。
- [x] 已有链接可编辑目标或删除；链接区域有可见的选中反馈。
- [ ] 保存后在 Preview、Adobe Acrobat Reader、Chrome 中可点击跳转。（手动验证项：真实阅读器矩阵未在本环境执行，见下方限制。）
- [x] 撤销/重做覆盖链接添加、编辑与删除。

## Verification notes

Merge commit `5f3c072` (`ticket/24-links` merged with `--no-ff` into `integration/pdfx`, no conflicts). Audit fix `5f213e7`. Close commit: `chore: close ticket 24`.

Independently verified behavior:

- Native structure: URL links are one `/Subtype /Link` with `/Rect`, `/QuadPoints` and `/A << /S /URI /URI ... >>`; Page links are `/A << /S /GoTo /D [page /XYZ null null null] >>` naming the target page object. Foreign URI, GoTo-action, direct `/Dest` and resolved named-`/Dest` links are read and editable; unrelated keys such as `/Border` survive. The private `PDFXLinkTarget` key and the `pdfx.invalid` placeholder are removed when the save pipeline materializes a Page link. JavaScript, Launch and unresolved links are listed with a reason and refused before any mutation.
- Geometry: link display rects/quads follow page `/Rotate` 90/180/270 with an offset `/CropBox` (hand-derived assertions in `link_rect_follows_page_rotation_and_crop_box`); multi-line selections keep one quadrilateral per line; image bounds use rotated corners clipped to the page. The clickable overlay uses the same quad rectangles as the visible selection.
- Page model: logical page numbers follow reorder, insertion and deletion; a link to an inserted blank page saves a valid destination and reopens. Deleting a target page reports the link as an unresolved destination instead of silently retargeting page 1; the saved GoTo is deliberately left dangling (ticket 26), and undo of the page deletion restores the target.
- Save: `serialize_live_document` remains the single pipeline; Page-link materialization runs inside `finalize_lopdf_revision`. A URL-only save appends one PDFium revision; a Page link appends exactly one lopdf revision on top (verified by xref-section counts, including with an inserted page's page-tree edit in the same revision). Repeated saves do not duplicate links; the Source Document's bytes and mtime are unchanged; flatten export keeps `/Link` native; existing annotations and AcroForm fields survive.
- History/permissions: create, target edit and delete are `EditAction::Link` in the unified history with undo/redo restoring target, region and metadata exactly; read-only documents refuse without mutating history or dirty state; link edits drive the existing dirty/close guard.
- Frontend: the link overlay is inert until armed and does not block text selection or other tools while off; link dialog validates http/https/mailto and page ranges with Chinese copy and cancel; the menu-state fix for a removed focused editable control keeps ticket 45 behavior.
- Security/privacy: URLs are stored text only and are never fetched, opened or executed; no telemetry/network additions; no dependency, vendor, spec, ADR, packaging or capability changes.

Key tests: `src-tauri/tests/links.rs` (17 tests, including foreign actions, duplicated `/NM` across pages, rotation/CropBox, page insert/delete/reorder, revision counts, read-only, repeat saves, forms); frontend `LinkLayer`, `LinkTargetDialog`, `LinkTextSelection`, `PageImageLayer`, `App`, `documentEvents`, `backend` and `linkTarget` suites.

Gate on the merged main repo: `npm run typecheck` passed; `npm test` 44 files / 531 tests passed; `npm run build` passed; `npm run test:e2e` 12 passed; `cargo fmt --check` passed; `cargo clippy -- -D warnings` passed with only the 4 vendored `pdfium-render` warnings; `cargo test` 366 passed / 3 ignored / 0 failed (links suite 17).

Limitations / manual items:

- Box 3 needs a real-reader click-through matrix (Preview, Adobe Acrobat Reader, Chrome), which this environment cannot run. The saved structure is asserted with an independent lopdf parse and a PDFium reopen.
- Foreign unsupported actions (JavaScript, Launch, remote/embedded GoTo) stay untouched and cannot be edited or deleted from PDFX; they are never executed.
- Deleting a page that is a link's target leaves the destination dangling by design. The link is then shown as unresolved (Chinese reason) and cannot be edited or deleted in-session; undo the page deletion to restore it.
- The link toolbar shares the top-right corner with pre-existing always-visible shape/form toolbars; this follows the existing overlay layout and needs a visual pass on narrow pages.
