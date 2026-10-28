# PDFX PDF engine technical spike (ticket 03)

Isolated, runnable probe for the PDFX M0→M1 engine decision: PDFium (`pdfium-render`
0.9.4) for rendering, text extraction and native annotations; `lopdf` 0.46 for
object-level edits and incremental appends; `rustybuzz` + `subsetter` + `ttf-parser`
for CJK Font Subset embedding.

This crate is **not** a workspace member and is **not** linked into the Tauri app.
It exists to produce evidence for [`docs/spikes/pdf-engine.md`](../../docs/spikes/pdf-engine.md).

## Result

All four probes pass on macOS (arm64) against PDFium 151.0.7881.0. The verdict is
**GO, conditional** on a small `pdfium-render` patch for incremental save (see the
go/no-go document). Summary of what the tests prove:

| Probe | Evidence |
|---|---|
| AC1 open/render/extract | text, scanned, encrypted (user + owner password), and bookmarked/linked structure fixtures open; rendered pixels written as PPM; per-character font/size/color/origin/bounds extracted |
| AC2 native annotations | highlight, underline, strikeout, note, ink, square, circle created, saved, reopened by PDFium; all seven types survive; annotations paint (pixel delta) |
| AC3 sequential save | PDFium incremental save then lopdf appended revision; each output is a byte-prefix of the next; bookmarks, URI link, Info title, XMP packet and attachment survive |
| AC4 CJK subset | Noto Sans SC 17,772,300 B → 4,268 B subset (16 glyphs) with non-identity `/CIDToGIDMap`, `/W` widths, ToUnicode; PDFium reopens and extracts `PDFX 中文字体测试 你好世界你好` exactly |

## Layout

```
spikes/pdf-engine/
├── Cargo.toml
├── scripts/setup-native.sh        # pinned PDFium + OFL font downloader (checksum-verified)
├── src/
│   ├── engine.rs                  # PDFium binding, open, render, per-char extraction
│   ├── raw.rs                     # raw FPDF_* calls missing from pdfium-render 0.9.4
│   ├── annotations.rs             # 7 annotation types + appearance streams + circle
│   ├── save_pipeline.rs           # PDFium incremental save → lopdf revision
│   ├── cjk_font.rs                # shape → subset → Type0/CIDFontType2 PDF writer
│   ├── lib.rs
│   └── main.rs                    # runnable probe, writes out/
└── tests/acceptance.rs            # one test per acceptance criterion
```

## Setup

Requirements: Rust 1.88+ (lopdf 0.46 is edition 2024), `curl`, `tar`, and a
platform PDFium binary supports (macOS arm64/x86_64, Linux x86_64, Windows x86_64).

```bash
cd spikes/pdf-engine
scripts/setup-native.sh            # add --force to re-download
```

The script pins and verifies:

| Asset | Version | SHA-256 | License |
|---|---|---|---|
| PDFium (`bblanchon/pdfium-binaries`) | 151.0.7881.0, tag `chromium/7881` | per-platform, in script | PDFium BSD-3-Clause; packaging MIT |
| Noto Sans SC VF (`google/fonts` commit `ddf386ba1b9fbc6b2c818f1ddcd9148f54ced94b`) | `NotoSansSC[wght].ttf` | `a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da` | SIL OFL 1.1 |

Nothing large is committed: `vendor/` and `out/` are gitignored. The PDFium
archive includes its `LICENSE` and full `licenses/` tree; the font's `OFL.txt`
is downloaded beside it. Provenance is written to `vendor/pdfium/SOURCE.txt` and
`vendor/fonts/SOURCE.txt`.

## Run

```bash
cd spikes/pdf-engine
cargo test                       # AC1–AC4 acceptance tests
cargo run --release              # runs all probes and writes out/
```

The probe resolves the native library from `PDFIUM_DYNAMIC_LIB_PATH` or
`vendor/pdfium/…`; the font from `PDFX_CJK_FONT` or `vendor/fonts/NotoSansSC-VF.ttf`.
Fixture lookup uses `PDFX_FIXTURES_DIR` or the repository's `fixtures/pdfs/`.
Tests fail loudly when an asset is missing — no probe is skipped silently.

### Outputs (`out/`, gitignored)

- `ac1/*.page-N.ppm`, `ac1/*.page-N.chars.tsv`, `ac1/summary.txt` — rendered pages and per-character extraction for every page of every AC1 fixture.
- `ac2/annotated.pdf`, `ac2/annotated.ppm`, `ac2/summary.txt` — the seven native annotations and before/after pixel counts.
- `ac3/sequential-save.pdf`, `ac3/summary.txt` — PDFium increment + lopdf revision, with prefix checks and surviving structure.
- `ac4/cjk-subset.pdf`, `ac4/NotoSansSC-subset.ttf`, `ac4/cjk-subset.ppm`, `ac4/summary.txt` — subset file, CID→GID→width→Unicode table, PDFium extraction.

## Known limits of `pdfium-render` 0.9.4 exposed by the probe

- `PdfDocument::save_to_*` hardcodes `flags = 0` (full rewrite); no incremental save.
  The spike calls `FPDF_SaveAsCopy(..., FPDF_INCREMENTAL)` through the raw bindings
  table on a separately loaded raw document.
- No public raw `FPDF_DOCUMENT`/`FPDF_PAGE`/`FPDF_ANNOTATION` handles.
- No `create_circle_annotation()` high-level creator (circle/ellipse is created via
  `FPDFPage_CreateAnnot`).
- No appearance-stream (`FPDFAnnot_SetAP`), border (`FPDFAnnot_SetBorder`) or ink
  stroke (`FPDFAnnot_AddInkStroke`) wrappers.
- PDFium does not paint newly created annotations without `/AP`; the probe installs
  simple Form XObject appearance streams for all seven types.
- AC2 therefore uses a high-level full rewrite as an intermediate before the raw
  incremental pass. Production must not do this; see the go/no-go for the required
  patch/raw-layer decision.

## License

Spike code follows the repository license. Dependencies are permissive:
`pdfium-render` MIT OR Apache-2.0, `lopdf` MIT, `rustybuzz` MIT, `subsetter` and
`ttf-parser` MIT OR Apache-2.0, `anyhow` MIT OR Apache-2.0. PDFium and Noto Sans SC
keep their own licenses as described above.
