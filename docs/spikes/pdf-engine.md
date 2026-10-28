# PDF engine technical spike — go/no-go

- **Status:** GO, conditional on one `pdfium-render` patch (see "M1 decisions")
- **Ticket:** 03 — M0→M1 technical verification
- **Branch:** `ticket/03-technical-spike`
- **Date:** 2026-10-28
- **Probe:** [`spikes/pdf-engine/`](../../spikes/pdf-engine/) (isolated crate, not a workspace member, not linked into the Tauri app)
- **Verification host:** macOS 26 (Darwin 25.6.0), arm64, Rust stable

## Verdict

The ADR-0010 hybrid engine is viable. PDFium 151.0.7881.0 through
`pdfium-render` 0.9.4 opens, renders and extracts every required fixture class;
all seven whitelisted native annotation types survive save/reopen and paint;
`FPDF_SaveAsCopy(FPDF_INCREMENTAL)` followed by a `lopdf::IncrementalDocument`
append preserves bookmarks, links, Info metadata, XMP and attachments; and a
CJK subset embeds with a non-identity `/CIDToGIDMap`, `/W`, ToUnicode and correct
GID remapping, with PDFium extracting the original string exactly.

**The one blocker is not PDFium — it is `pdfium-render` 0.9.4's save API.**
`PdfDocument::save_to_writer` hardcodes `flags = 0` (full rewrite) and the
document handle is crate-private. A production save cannot be incremental
through the high-level API. M1 must vendor a small patch/fork of
`pdfium-render` (MIT OR Apache-2.0) that exposes the raw document handle and a
save-with-flags entry point, or route all writes through an internal raw layer
like the one in this spike. Without that, every annotation edit rewrites the
whole file and ADR-0004 is violated.

No probe failed. The items under "Known limits" are untested paths or
`pdfium-render` gaps with defined workarounds, not observed failures.

## Pinned stack

| Component | Version | License |
|---|---|---|
| PDFium shared library | **151.0.7881.0**, `bblanchon/pdfium-binaries` release tag `chromium/7881` | BSD-3-Clause (packaging MIT) |
| `pdfium-render` | **0.9.4** (`pdfium_latest` → `pdfium_7881`) | MIT OR Apache-2.0 |
| `lopdf` | **0.46.0** (edition 2024, MSRV 1.88) | MIT |
| `rustybuzz` | **0.20.1** | MIT |
| `subsetter` | **0.2.6** (`variable-fonts` on by default) | MIT OR Apache-2.0 |
| `ttf-parser` | **0.25.1** (via `rustybuzz`; RUSTSEC-2026-0192 unmaintained, no memory-safety issue) | MIT OR Apache-2.0 |
| `anyhow` | **1.0.104** | MIT OR Apache-2.0 |
| Noto Sans SC VF | `google/fonts` commit `ddf386ba1b9fbc6b2c818f1ddcd9148f54ced94b`, `NotoSansSC[wght].ttf` | SIL OFL 1.1 |

PDFium 7881 is the binary `pdfium-render` 0.9.4 declares support for, and it
contains the incremental-save fix (Chromium issue 42270523). A newer 8125 build
was also exercised successfully during the spike, but 7881 is the pin M1 should
carry until the bindings are upgraded.

## Reproduction

```bash
cd spikes/pdf-engine
scripts/setup-native.sh          # downloads + SHA-256-verifies PDFium and Noto Sans SC
cargo test                       # 4 acceptance tests (AC1–AC4)
cargo run --release              # runs the probes, writes out/, prints a summary
```

The setup script pins platform assets and checksums:

| Platform | Asset | SHA-256 |
|---|---|---|
| macOS arm64 / x86_64 | `pdfium-mac-univ.tgz` | `df451a413c3609585e84a4a91110a9bc889cff05fe3b2db0ed817c9e90c3f7d3` |
| Linux x86_64 | `pdfium-linux-x64.tgz` | `1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d` |
| Windows x86_64 | `pdfium-win-x64.tgz` | `73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08` |

Noto Sans SC: `a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da`.
Assets are gitignored; only the script, checksums and license notes are committed.
The archive retains PDFium's `LICENSE` and `licenses/` tree; `OFL.txt` is fetched
next to the font. Source URLs and hashes are written to `vendor/*/SOURCE.txt`.

Fixture lookup defaults to `fixtures/pdfs/` (override with `PDFX_FIXTURES_DIR`).
Native library path defaults to `vendor/pdfium/…` (override with
`PDFIUM_DYNAMIC_LIB_PATH`); font path defaults to `vendor/fonts/NotoSansSC-VF.ttf`
(override with `PDFX_CJK_FONT`). Missing assets fail the tests, never skip them.

## Probe results

Command: `cargo run` on the pins above. Full output is in `out/` after a run.

### AC1 — open, render, extract

| Fixture | Pages | Extracted chars | Render (1:1) | Non-white / dark px |
|---|---|---|---|---|
| `text-latin.pdf` | 1 | 127 | 612×792 | 7,079 / 3,389 |
| `scanned-image-only.pdf` | 1 | 0 (no text layer) | 612×792 | 484,704 / 147,665 |
| `encrypted-user-password.pdf` (password `pdfx-user`) | 1 | 118 | 612×792 | 5,697 / 2,421 |
| `structure-metadata.pdf` | 1 | 70 | 612×792 | 3,360 / 1,460 |

- Wrong user password fails to open; empty-password owner-restricted file opens
  with the expected permission flags (`can_modify_document_content`,
  `can_add_or_modify_text_annotations`, `can_extract_text_and_graphics` false;
  high-quality print true — the fixture is R2/V1, where quality flags collapse).
- Per-character extraction returns font name (`Helvetica` on the text fixture),
  18 pt size, black fill, baseline near (72, 720), and a non-empty loose box.
- Output: per-page PPM renders and TSV (`unicode, font, size, color, origin,
  bounds`) for every page of every fixture.

### AC2 — whitelisted native annotations

High-level `pdfium-render` creates highlight, underline, strikeout, note (Text),
ink and square. Circle/ellipse has no high-level creator in 0.9.4 and is created
with `FPDFPage_CreateAnnot(page, FPDF_ANNOT_CIRCLE)`.

After the raw pass installs `/AP` appearance streams and the circle, an
incremental PDFium save is appended. Reopening with PDFium shows all seven:
`["Highlight", "Underline", "Strikeout", "Text", "Ink", "Square", "Circle"]`.
Rendered non-white pixels rise from **7,079 to 22,979** (+15,900), so the
annotations are not merely present in the object graph — they paint.

Important finding: PDFium does **not** generate appearances for these
annotations. Without `/AP`, all seven exist in the file and survive reopen but
are invisible. `FPDFAnnot_SetAP` creates a Form XObject whose `/BBox` is the
annotation `/Rect`, and the content stream uses page coordinates. M1 must
generate appearance streams (or use a patched helper that does).

### AC3 — sequential incremental save

`structure-metadata.pdf` (2,206 B, classic xref table) → PDFium
`FPDF_SaveAsCopy` with `FPDF_INCREMENTAL` (**4,018 B**, original bytes are an
exact prefix) → `lopdf::IncrementalDocument` append of one cloned Info object
(**4,355 B**, PDFium bytes are an exact prefix).

Both engines' changes are visible in the final file:

- PDFium: one Square annotation with `/AP` on page 1 (page already had the Link
  annotation).
- lopdf: `PDFXSpike = (lopdf-append)` in `/Info`.
- Still present after reopen: bookmarks `Introduction` and `Details`, URI link
  `https://example.com/pdfx`, title `PDFX structure fixture`, the XMP packet and
  the embedded attachment.

This confirms the recommended order — **PDFium first, lopdf second** — and that
`lopdf` appends onto PDFium's revision rather than the source. It also validates
the `create_from(pdfium_bytes, parsed)` + `opt_clone_object_to_new_document` +
`save_to` recipe from the setup notes.

### AC4 — CJK Font Subset

Text: `PDFX 中文字体测试 你好世界你好`, 24 pt.

- Noto Sans SC VF: **17,772,300 B → 4,268 B** subset, **16 glyphs** (15 used +
  `.notdef`).
- The probe deliberately uses the **old glyph ID as the CID** and writes a
  **non-identity `/CIDToGIDMap` stream**, e.g. 中: CID 8805 → GID 6, 世:
  CID 8782 → GID 14, space: CID 1 → GID 5. `/W` and ToUnicode are keyed by the
  same CID.
- Independent width check: every CJK ideograph has width 1000/1000 em; Latin
  widths are 496–661; space is 220.
- PDFium reopens the saved file and extracts exactly
  `PDFX 中文字体测试 你好世界你好`; per-character extraction preserves the string
  with no U+FFFD, font size 24 and the `PDFX`-prefixed font name.
- The rendered page has 2,572 non-white pixels, so the subset glyphs are
  drawable, not just extractable.

The non-identity map proves PDFium honours `/CIDToGIDMap` streams. For M1 the
simpler equivalent is recommended: use the **new** GID (from `GlyphRemapper`)
as CID and `/CIDToGIDMap /Identity`. Key content codes, `/W` and ToUnicode by
that same new GID. Do not mix old and new GIDs. Subset once per document per
face/weight, not per text box.

## M1 decisions

### Save

1. Patch/fork `pdfium-render` 0.9.4 to expose `FPDF_SaveAsCopy` flags (and,
   preferably, the raw document handle or a `PdfDocument::save_to_writer_with_flags`).
   Pin the fork by git revision; upstream the change if possible. A minimal
   patch is preferable to maintaining a parallel raw engine.
2. Save order: all PDFium edits → incremental PDFium save → `lopdf` parses those
   bytes → one `IncrementalDocument` append → atomic temp-file write + fsync +
   rename. One writer per revision; never save the same `IncrementalDocument`
   twice. Reopen PDFium from the saved bytes before the next edit session.
3. The spike's AC2 workaround (high-level full rewrite as an intermediate) must
   **not** ship. It exists only because 0.9.4 cannot save high-level annotation
   edits incrementally.
4. For encrypted inputs: PDFium's incremental save preserves `/Encrypt`, while
   ADR-0007 requires unencrypted output. Do one full rewrite with
   `FPDF_REMOVE_SECURITY` first, then append the lopdf revision to those bytes.
   This path was **not exercised** by the spike (no encrypted save fixture).

### Annotations

- Create markup with high-level creators where present; use raw FFI for
  circle/ellipse, appearance streams, border width and ink strokes.
- Generate `/AP` for every created annotation; otherwise PDFium and other
  viewers show nothing. The probe's simple streams are a template, not a
  production appearance generator.
- Ink is a special case: the high-level creator produces an empty ink
  annotation. The probe attaches a page-object line and an `/AP` so the result
  is visible, but M1 needs `FPDFAnnot_AddInkStroke` (plus border width) to make
  the annotation editable as an ink annotation.
- Reopen and render assertions are required in tests: object presence alone does
  not prove visibility.

### Fonts

- Use Type0 / Identity-H with a CIDFontType2 descendant, FontDescriptor,
  FontFile2 and ToUnicode — **not** CIDFontType0/CFF.
- Use Noto Sans SC / Noto Serif SC **variable TrueType** builds. ADR-0006 now
  specifies these TrueType builds; Adobe's Source Han OTF releases are CFF and
  would require a separate CIDFontType0/CFF pipeline.
- Shape with `rustybuzz`, subset with `subsetter`, and treat the old→new GID map
  as the single source of truth for content codes, `/W` and ToUnicode.
- Use `subset_with_variations` for non-default weights (bold/opsz). The probe
  only exercised the default instance.
- Subset once per face/weight per document and reuse the font resource across
  pages.

## Known limits / risks for follow-up tickets

| Risk | Status | Mitigation |
|---|---|---|
| `pdfium-render` 0.9.4 cannot incrementally save high-level edits | Confirmed | Vendored patch/fork exposing save flags + handle, or an internal raw save layer |
| New annotations are invisible without `/AP` | Confirmed | Appearance-stream generator; verify by rendering |
| Circle/ellipse, border, ink strokes lack high-level APIs | Confirmed | Raw FFI (`FPDFPage_CreateAnnot`, `FPDFAnnot_SetBorder`, `FPDFAnnot_AddInkStroke`) |
| Encrypted source → unencrypted incremental output | Not probed | One `FPDF_REMOVE_SECURITY` full rewrite, then lopdf append |
| Xref-stream inputs (PDF 1.5+) | Not probed (fixtures use classic xref tables) | Add an xref-stream fixture; lopdf supports both xref types |
| Non-default variable-font weights | Not probed | `subsetter::subset_with_variations` with explicit `wght` |
| Font reuse across pages / many text boxes | Not probed at scale | Subset once per document per face; measure 500-page fixture |
| 200 MB / 2,000-page memory ceiling | Not measured | Instrument save pipeline; avoid concurrent full copies |
| \(pdfium-render\) thread-safe feature serializes all calls | Known | Dedicated engine thread; do not expect parallel rendering |
| RUSTSEC-2026-0192 (`ttf-parser` unmaintained) | Known, no memory-safety issue | Pin; run `cargo deny`; track `skrifa` |

## Exact commands that produced this verdict

```bash
cd spikes/pdf-engine
scripts/setup-native.sh --force
cargo test
cargo run --release
```

`cargo test` result: 4 passed, 0 failed (AC1–AC4).
`cargo run` result: all probes completed (`SPIKE OK`).
