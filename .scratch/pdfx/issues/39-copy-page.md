# 39: 复制页面

**What to build:** 用户可以复制当前 Page，副本插入到原页之后，缩略图与页码立即更新，保存后页序正确。

**Blocked by:** 26 (页面操作：旋转/删除/重排/插入空白页)

**Status:** done

- [x] 复制当前 Page 并插入到原页之后，内容与尺寸一致。
- [x] 副本的 Annotation、链接与 Form Field 行为与原页一致（或明确说明共享/独立策略）。
- [x] 保存后页数与页序正确，未操作页面内容保留。
- [x] 撤销/重做覆盖复制操作。

## Verification notes

Merge commit `0026d68` (`ticket/39-copy-page` merged with `--no-ff` into `integration/pdfx`, no conflicts). Audit fix `9fbe1e5`; branch tip before the merge was `9fbe1e5` over `6089007`, `05e9da2` and `3617c08`. Close commit: `chore: close ticket 39`.

Independently verified behavior:

- Content/size: `copy_inserts_after_current_with_identical_content_size_rotation_and_links` rotates the source page, copies it, and checks the live page info (same width/height, rotation 90, copy at logical slot 1), the extracted text layer, and the saved independent lopdf parse (4 pages, identical `MediaBox`, identical rotation on the pair, untouched fourth page keeps its rotation). `copied_page_keeps_the_crop_box_and_crop_relative_geometry` checks an offset `CropBox`: the copy keeps the source MediaBox/CropBox, display size, crop origin and the full character layer, live and saved. The copy is a real appended physical page; blank-page insertion is untouched.
- Annotation policy: copied annotations are independent copies with fresh `/NM` identities. The importer copies `/NM` verbatim, so without the fix a copy and its source both answered to the same name and name-addressed delete/edit resolved the source annotation first. `refresh_copied_annotation_names` gives every named, non-widget imported annotation a fresh user-visible identity immediately after import; unnamed annotations stay unnamed (same editability as their source). `copied_annotations_get_fresh_names_and_deleting_the_copy_keeps_the_source` shows two distinct square annotations after copy, both names surviving save/reopen, deleting the copy's name leaving the source annotation intact, and the saved output containing only the source square afterwards.
- Link policy: copies keep the source's link targets, live and after save. `copied_page_keeps_link_targets_live_and_after_save` covers three page destinations and one unsupported destination; `retargeting_a_copied_link_leaves_the_source_link_untouched` proves a URI chosen on the copy is not overwritten by the save fixup. The new `copied_links_follow_pages_moved_after_the_copy` moves the destination page to slot 0 after copying and reasserts the logical targets after save/reopen; `copied_links_to_a_deleted_page_stay_visibly_unsupported` deletes the target page and confirms the four links on both remaining pages stay visibly unsupported rather than silently retargeting page 0, live and saved. Copied links are matched by rectangle with order fallback, so unrelated links are not resurrected.
- Form-field policy: copied widgets are independent, namespaced fields (`<raw>__copy_p<physical>`), filled through the session overlay so writes never reach the shared PDFium field object; the save pipeline writes the copy's own `/V`/`/AS` and cloned parent field. `copied_form_page_is_independent_through_save_and_reopen` covers text, checkbox, radio group, dropdown and list box through save/reopen; `copying_a_copied_form_page_keeps_names_unique_and_values_independent` covers copy-of-copy names; `undo_and_redo_cover_the_copy_and_its_form_edits` covers undo/redo of fills on the copy. The copy starts with the source page's current session-visible values (including radio/choice overlay values) and then diverges.
- Session-created fields (tickets 31-33): `copying_a_page_duplicates_session_created_fields_with_independent_values` creates a text field, fills it, copies the page, and asserts the clone `session_field__copy_p2` appears immediately with the source's value and `created: true`. Undoing the copy hides the clone with the page and redo restores it (the clone is not a separate history entry; the page action owns it). Editing the clone and then the source writes independent `/V` values, and the saved file materializes two distinct root fields, verified by an independent lopdf parse plus a PDFium reopen.
- Nested AcroForm: the audit found a save failure for hierarchical fields (`person.name`): the fixup renamed the cloned parent with its shallow `/T` and did not append it to `/Fields`, so the inherited-value `SetTextValue` could not find `person.name__copy_p1`. `apply_copy_fields_fixup` now builds the full dotted name from the `/Parent` chain, flattens the cloned parent into a self-contained root field (inheritable `FT/Ff/V/DV/AA/DA/Q/DS/RV/Opt/MaxLen` copied from ancestors, `/Parent` removed), and appends it to `/AcroForm /Fields`. `copying_a_page_with_nested_acrofom_fields_saves_independent_fields` builds a synthetic nested PDF, copies the page, fills the copy, saves and reopens: the source stays `Ada`, the copy reads `Copy Ada`, and the saved roots are `person` and `person.name__copy_p1`.
- Save/history: copying stays inside the existing ticket-10 pipeline (one PDFium serialization, at most one lopdf revision). `repeat_saves_of_a_copied_page_are_stable` covers repeat saves; the Source Document's bytes/mtime are asserted unchanged in the content and form tests. The copy is one page action in the unified history; undo/redo hides and restores the page, its fields and its values. Read-only documents refuse the command with the Chinese permission error without touching history. Annotation copies keep separate annotation objects in the saved file (asserted in the first test).
- Audit decisions: `copy_snapshots` keeps one PDFium serialization per copied page for the document lifetime because imported widgets with `/Parent` point into the source snapshot; the memory cost is documented on the field. No new dependencies, network/telemetry, spec/ADR/packaging/capability changes; the vendor tree is unchanged.

Key tests: `src-tauri/tests/copy_page.rs` (14 tests, including the five added by the audit fix); full `cargo test` 380 passed / 3 ignored / 0 failed; frontend suite unaffected.

Gate on the merged main repo: `npm run typecheck` passed; `npm test` 44 files / 533 tests passed; `npm run build` passed; `npm run test:e2e` 12 passed; `cargo fmt --check` passed; `cargo clippy -- -D warnings` passed with only the 4 vendored `pdfium-render` warnings; `cargo test` 380 passed / 3 ignored / 0 failed (copy_page suite 14).

Limitations / manual items:

- Copied-widget appearance streams: filling an existing widget updates `/V` and `/AS` at save but does not regenerate the imported `/AP`. An audit probe showed the same stale stream on the *original* fixture field (`full_name` still painted `Ada Lovelace` after editing), so this is a pre-existing ticket-30 fill limitation, not copy-specific; created fields (31-33) still write explicit `/AP` with no `/NeedAppearances`. Some viewers trust `/V` and render the edit; others may show the old appearance. A manual Preview/Adobe/Chrome matrix is still required.
- Nested source fields: `fill_form_field` on an existing nested field (`person.name` in a parent/child chain) is a silent no-op in the pre-existing PDFium fill path even without a copy (verified by probe: the command returns Ok but `/V` stays `Ada`). The copied nested field works because the copy is filled through the overlay and saved by the fixup. Fixing nested source filling is outside ticket 39.
- Field-name collisions: the copy namespace is `<raw>__copy_p<physical>`. A source PDF that already contains a literal field with that name could collide; this is adversarial and not handled.
- Multi-page field groups: a field whose widgets span several copied pages is cloned per copied page (same split as before); the fixture set does not cover that shape.
- Unnamed pre-existing annotations are not listed or editable in PDFX; copies keep that behavior.
- Memory: one document-sized PDFium snapshot is retained per copied page until the document closes.
- The packaged app and a real-reader visual matrix (Preview, Adobe Acrobat Reader, Chrome, Edge) were not run in this environment; saved structure was asserted with an independent lopdf parse and a PDFium reopen.
