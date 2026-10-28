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

`pdfium-render` 0.9.4 always passes save flags `0` to `FPDF_SaveAsCopy()` and
does not expose circle-annotation creation, ink strokes, annotation border
width or custom appearance streams. PDFX needs those capabilities on the *same*
`PdfDocument` that holds the high-level `pdfium-render` edits, so that one
save writes every edit in one revision (see `docs/spikes/pdf-engine.md`, AC2 —
the spike's "rewrite, then reopen with a second raw document" workaround must
not ship).

Vendoring with a minimal patch keeps the high-level document, its edit state
and the low-level operations on one handle. No `FPDF_DOCUMENT` handle is
exposed by the patch: `PdfDocument::handle()` remains `pub(crate)`, and the
safe wrapper lives in `src-tauri/src/pdfium_adapter.rs`.

## Patch scope

Four files change. Each hunk is additive; no upstream behaviour changes.

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

### `src/pdf/document/page/annotation/ink.rs`

- Added `PdfPageInkAnnotation::add_ink_stroke(&[(f32, f32)])` calling
  `FPDFAnnot_AddInkStroke()`; returns the new zero-based stroke index.
- Added `PdfPageInkAnnotation::ink_stroke_count()` calling
  `FPDFAnnot_GetInkListCount()`.

### `src/pdf/document/page/annotation.rs`

Added to the `PdfPageAnnotation` enum (available on every annotation type):

- `border_width()` — `FPDFAnnot_GetBorder()`.
- `set_border_width(width)` — `FPDFAnnot_SetBorder(annot, 0.0, 0.0, width)`.
  Pdfium drops an existing appearance stream when the border is set, so PDFX
  always sets the border before the appearance.
- `set_appearance(stream)` — `FPDFAnnot_SetAP_str()` with
  `FPDF_ANNOT_APPEARANCEMODE_NORMAL`. Pdfium builds a Form XObject whose
  `/BBox` is the annotation `/Rect` at call time, so bounds must be set first.
- `add_ink_stroke(points)` / `ink_stroke_count()` — delegate to the ink
  variant, returning `PdfiumError::UnknownPdfAnnotationType` otherwise.

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

## Upgrade procedure

1. Fetch the new `pdfium-render` release from crates.io and record its `.crate`
   sha256 here.
2. Replace everything in this directory with the new crate contents plus
   `LICENSE.md`, keeping `PDFX-PATCH.md`.
3. Re-apply the four hunks above. `diff -u` against the pristine crate source
   (cargo registry unpack directory or a fresh extract of the `.crate`) is the
   authoritative patch; keep the diff to these four files.
4. Update `src-tauri/Cargo.toml` version if the patched range changes, then
   build. `pdfium-render` tracks the PDFium API, so re-run the adapter tests
   against the pinned PDFium build first.
5. If upstream gains flags-aware saves or the annotation methods, delete the
   vendor directory and the `[patch.crates-io]` entry instead of carrying a
   no-op patch.

## Known limitations

- Upstream concrete annotation types (e.g. `PdfPageCircleAnnotation`) have no
  `Drop`; only the `PdfPageAnnotation` enum closes its annotation handle. This
  is upstream behaviour, unchanged by the patch. The adapter's creation helpers
  therefore return only an `AnnotationRef`, and later operations use the enum,
  which does close the handle.
- The patch adds no raw-handle accessor. Any future capability must be added
  here with a typed, safe method rather than exposing `FPDF_*` handles.
