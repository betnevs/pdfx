# pdfium-render 0.9.4 — PDFX vendor patch

PDFX vendors `pdfium-render` with a small, isolated patch. Everything else in
this directory is the unmodified 0.9.4 crate from crates.io.

## Pin

| | |
|---|---|
| Crate | `pdfium-render` 0.9.4 |
| Source | crates.io |
| Package checksum (sha256 of `pdfium-render-0.9.4.crate`) | `8948a803616a9e936b15a6637af2cd48c5fb8ae0fcdeb3c32eac3a540e255a19` |
| License | MIT OR Apache-2.0 (`LICENSE.md`) |
| Upstream | <https://github.com/ajrcarey/pdfium-render> |
| PDFium build | 151.0.7881.0 (see `scripts/setup-native.sh`) |

The vendored copy contains the crate's build inputs (`Cargo.toml`, `build.rs`,
`config.toml`, `src/`, `README.md` — required by `src/lib.rs`'s
`#![doc = include_str!("../README.md")]`) and `LICENSE.md`. The upstream
`include/` C headers, tests and packaging metadata are not needed to build the
crate as a path dependency and were left out.

Wiring in `src-tauri/Cargo.toml`:

```toml
[patch.crates-io]
pdfium-render = { path = "vendor/pdfium-render" }
```

`Cargo.lock` therefore records the dependency as a path dependency; the
checksum above identifies the upstream release this directory started from.

## Why vendor

`pdfium-render` 0.9.4 always passes save flags `0` to `FPDF_SaveAsCopy()`,
does not expose circle-annotation creation, ink strokes, annotation border
width or custom appearance streams, and cannot load a CIDFontType2 from
caller-supplied font bytes, ToUnicode CMap and CIDToGIDMap
(`FPDFText_LoadCidType2Font()`), which PDFX needs for its `subsetter`-produced
CJK font subsets. PDFX needs those capabilities on the *same*
`PdfDocument` that holds the high-level `pdfium-render` edits, so that one
save writes every edit in one revision (see `docs/spikes/pdf-engine.md`, AC2 —
the spike's "rewrite, then reopen with a second raw document" workaround must
not ship).

Vendoring with a minimal patch keeps the high-level document, its edit state
and the low-level operations on one handle. No `FPDF_DOCUMENT` handle is
exposed by the patch: `PdfDocument::handle()` remains `pub(crate)`, and the
safe wrapper lives in `src-tauri/src/pdfium_adapter.rs`.

## Patch scope

Ten files change. Each hunk is additive; no upstream behaviour changes.

### `src/pdf/document.rs`

- `use crate::bindgen::{FPDF_DOCUMENT, FPDF_DWORD};`
- Extracted the body of `save_to_writer()` into
  `save_to_writer_with_flags(&self, writer, flags)`; `save_to_writer()` now
  calls it with flags `0`, preserving upstream behaviour.
- Added `save_to_file_with_flags(path, flags)` (non-wasm).
- Added `save_to_bytes_with_flags(flags)`.
- Added associated constants `FPDF_INCREMENTAL = 1`,
  `FPDF_NO_INCREMENTAL = 2`, `FPDF_REMOVE_SECURITY = 4`.

### `src/pdf/document/page/annotations.rs`

- Added `create_circle_annotation()`, mirroring the other
  `create_*_annotation()` constructors (creation date, content regeneration).

### `src/pdf/document/page/annotation/circle.rs`

- Added `PdfPageCircleAnnotation::close(self)` calling
  `FPDFPage_CloseAnnot()`. Upstream annotation types do not implement `Drop`,
  so Pdfium's requirement to close handles returned by `FPDFPage_CreateAnnot()`
  has no other safe outlet.

### `src/pdf/document/page/annotation/ink.rs`

- Added `PdfPageInkAnnotation::close(self)` calling `FPDFPage_CloseAnnot()`.
- Added `PdfPageInkAnnotation::add_ink_stroke(&[(f32, f32)])` calling
  `FPDFAnnot_AddInkStroke()`; returns the new zero-based stroke index.
- Added `PdfPageInkAnnotation::ink_stroke_count()` calling
  `FPDFAnnot_GetInkListCount()`.

### `src/pdf/document/page/annotation.rs`

Added to the `PdfPageAnnotation` enum (available on every annotation type):

- `name()` / `set_name(name)` — `/NM` string value, the stable annotation
  identity PDFX uses across edits and undo/redo.
- `creation_date()` / `set_creation_date_string(date)` — raw `/CreationDate`
  string access. Unlike upstream's date-typed `set_creation_date()`, undo can
  restore the timestamp it read without constructing a new one.
- `string_value(key)` / `set_string_value(key, value)` — arbitrary annotation
  dictionary string values. PDFX mirrors colour and opacity in private keys so
  it can read them back after Pdfium installs an appearance stream.
- `border_width()` — `FPDFAnnot_GetBorder()`.
- `set_border_width(width)` — `FPDFAnnot_SetBorder(annot, 0.0, 0.0, width)`.
  Pdfium drops an existing appearance stream when the border is set, so PDFX
  always sets the border before the appearance.
- `set_appearance(stream)` — `FPDFAnnot_SetAP_str()` with
  `FPDF_ANNOT_APPEARANCEMODE_NORMAL`. Pdfium builds a Form XObject whose
  `/BBox` is the annotation `/Rect` at call time, so bounds must be set first.
- `add_ink_stroke(points)` / `ink_stroke_count()` — delegate to the ink
  variant, returning `PdfiumError::UnknownPdfAnnotationType` otherwise.

### `src/pdf/document/page/annotation/highlight.rs`

- Added `PdfPageHighlightAnnotation::close(self)` calling
  `FPDFPage_CloseAnnot()`.

### `src/pdf/document/page/annotation/underline.rs`

- Added `PdfPageUnderlineAnnotation::close(self)` calling
  `FPDFPage_CloseAnnot()`.

### `src/pdf/document/page/annotation/strikeout.rs`

- Added `PdfPageStrikeoutAnnotation::close(self)` calling
  `FPDFPage_CloseAnnot()`.

### `src/pdf/document/page/annotation/stamp.rs`

- Added `PdfPageStampAnnotation::close(self)` calling
  `FPDFPage_CloseAnnot()`.

### `src/pdf/document/fonts.rs`

- Added `PdfFonts::load_cid_type2_from_bytes(font_data, to_unicode_cmap,
  cid_to_gid_map_data)` calling `FPDFText_LoadCidType2Font()`. It lets PDFX
  embed a `subsetter`-produced CIDFontType2 together with the caller's
  ToUnicode CMap and CIDToGIDMap instead of letting Pdfium regenerate them
  from the font program. The new font is registered in the document's font
  collection like the other `load_*_from_bytes()` constructors.
- Added `PdfFontToken::to_usize()` / `PdfFontToken::from_usize(handle)`. The
  edit session caches bundled-font tokens by character set and must stay
  `Send` for the global document registry; a raw pointer would not. The token
  remains a non-owning handle valid only while its document is alive, and
  PDFX stores it only in that document's session (dropped first).

## Verification

The patch is exercised through the safe adapter in
`src-tauri/src/pdfium_adapter.rs` by `src-tauri/tests/pdfium_adapter.rs`:

```sh
cd src-tauri
cargo test --test pdfium_adapter
```

| Test | What it proves |
|---|---|
| `incremental_save_keeps_original_bytes_as_prefix_and_preserves_structure` | Output starts with the source bytes; bookmarks, URI link, metadata, attachment, text and a high-level square annotation survive reopen. |
| `full_rewrite_preserves_document_structure_and_high_level_edits` | `FPDF_NO_INCREMENTAL` is a new file, not an append, and retains the same bookmarks, link, metadata, attachment, text and high-level edit. |
| `circle_annotation_with_border_and_custom_appearance_is_visible_after_reopen` | Circle creation, border width 4.0 round-trip, `FPDFAnnot_SetAP` stream is rendered after save/reopen. |
| `ink_annotation_strokes_and_appearance_survive_save_and_reopen` | Two strokes added to `/InkList`, count round-trips, custom appearance is rendered. |
| `encrypted_sources_refuse_encrypted_output_and_remove_security_decrypts` | Incremental/full-rewrite saves of encrypted fixtures are refused; `FPDF_REMOVE_SECURITY` output reopens without a password with `PdfSecurityHandlerRevision::Unprotected`. |
| `full_rewrite_annotates_only_the_target_page_and_preserves_untouched_content` | `FPDF_NO_INCREMENTAL` output is not an append, keeps all 500 pages, paints the new annotation, and leaves an untouched page's rendered pixels byte-identical. |

Run the complete Rust suite with:

```sh
cd src-tauri
cargo check
cargo test
```

The CID-font additions are covered by `src-tauri/tests/cjk_fonts.rs`, which
drives the command boundary, saves the document and reopens it with lopdf and
PDFium to assert the Type0/CIDFontType2 structure, `/W` widths, CIDToGIDMap,
ToUnicode text extraction and repeat-save stability:

```sh
cd src-tauri
cargo test --test cjk_fonts
```

## Upgrade procedure

1. Fetch the new `pdfium-render` release from crates.io and record its `.crate`
   sha256 here.
2. Replace everything in this directory with the new crate contents plus
   `LICENSE.md`, keeping `PDFX-PATCH.md`.
3. Re-apply the hunks above (ten files). `diff -u` against the pristine crate
   source (cargo registry unpack directory or a fresh extract of the `.crate`)
   is the authoritative patch; keep the diff to those ten files.
4. Update `src-tauri/Cargo.toml` version if the patched range changes, then
   build. `pdfium-render` tracks the PDFium API, so re-run the adapter tests
   against the pinned PDFium build first.
5. If upstream gains flags-aware saves or the annotation methods, delete the
   vendor directory and the `[patch.crates-io]` entry instead of carrying a
   no-op patch.

## Known limitations

- Upstream concrete annotation types (e.g. `PdfPageCircleAnnotation`) have no
  `Drop`; only the `PdfPageAnnotation` enum closes its annotation handle. The
  patch adds `close(self)` to the six types PDFX creates (circle, ink,
  highlight, underline, strikeout, stamp), and the adapter calls it immediately
  after setting bounds. Other concrete annotation types keep upstream behaviour, so
  callers creating them directly must close them explicitly if the patch is
  extended to expose them. Later adapter operations fetch the annotation
  through the enum, which does close the handle.
- The patch adds no raw-handle accessor. Any future capability must be added
  here with a typed, safe method rather than exposing `FPDF_*` handles.
