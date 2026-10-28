# AcroForm creation technical spike — go/no-go

- **Status:** GO, with rotation/CJK appearance work carried into implementation
- **Ticket:** 29 — AcroForm 创建技术验证
- **Branch:** `ticket/29-acroform-spike`
- **Date:** 2026-10-28
- **Probe:** [`spikes/acroform-create/`](../../spikes/acroform-create/) (isolated crate, not a workspace member, not linked into the Tauri app)
- **Verification host:** macOS 26 (Darwin 25.6.0), arm64, Rust stable, Chrome 156.0.8078.48, PDFium 151.0.7881.0, `pdfium-render` 0.9.4, `lopdf` 0.46

## Verdict

Creating AcroForms with `lopdf` is viable. The probe creates text, checkbox,
radio group, list, combo/dropdown and empty signature fields; PDFium recognizes
every field and state; Chrome paints every field that has an appearance and
successfully edits a text field and a checkbox; PDFKit (Preview's PDF engine)
paints the same fields and reads back every field's value and state (the empty
signature field has structure but no visible box). Merging into an existing
AcroForm preserves the old fields, values and page content, and flattening
produces a form-free copy whose rendered page is pixel-identical to the
interactive form.

Tickets 31–33 can proceed. The estimate in the spec (2–4 engineer weeks) still
holds, with rotation-aware appearance streams and CJK appearance fonts as
explicit work items. The fallback “只填不建” is not required.

Two reader caveats must stay visible:

- Chrome display and fill were verified. **Preview.app's UI was not verified**
  because the GUI session was locked (`CGSSessionScreenIsLocked = Yes`) and
  `screencapture` returned a black image. PDFKit, the engine behind Preview,
  was used as the programmatic proxy.
- **Adobe Acrobat Reader is not installed** on this host and was not tested.

## What was proven

- `/AcroForm`, `/Fields`, `/DA`, `/DR /Font`, widget annotations and per-field
  appearance streams are enough for readers to display and fill generated
  fields. No reader-generated appearance is required.
- Text: `/FT /Tx`, required flag, `/V`, `/DA`, WinAnsi Helvetica appearance,
  and UTF-16BE `/V` for non-ASCII values.
- Checkbox: `/AP /N` has `Off` and `Yes` states, with `/V` and `/AS` in sync.
- Radio: a parent field with `/Kids`, one widget per option, unique on-state
  export values, and exactly one kid selected through `/AS` and parent `/V`.
- List and combo: `/Opt`, `/V`, and `/I`; the list multi-select flag and
  multi-index path are implemented, while the committed fixture selects one
  row. Appearance streams show the selected rows/value.
- Empty signature: an `/AcroForm` field with `/FT /Sig` and `SigFlags` bit 1.
  No certificate signing and no appearance stream.
- Merge into an existing AcroForm using `lopdf::IncrementalDocument`: existing
  fields, `/DA`, `/DR`, `/NeedAppearances` and page content survive; name
  collisions get `_2`, `_3` suffixes; the original bytes are an exact prefix.
- Flatten: widget appearances are baked into page content, `/AcroForm` and
  widget annotations are removed, and PDFium renders the same pixels before and
  after.
- Widget `/Rect` mapping for `/Rotate` 0/90/180/270 with and without a CropBox
  is correct and covered by a geometry test.

## Reproduction

```bash
cd spikes/acroform-create
scripts/setup-native.sh     # reuses spikes/pdf-engine/vendor/pdfium when present
cargo run                   # deterministic fixture generation
cargo test                  # 15 tests
cargo fmt --check
```

The probe needs the PDFium dynamic library used by the pdf-engine spike. If it
is absent, `scripts/setup-native.sh` delegates to
`spikes/pdf-engine/scripts/setup-native.sh`; the vendor directory is gitignored.

Fixture generation is deterministic: running `cargo run` twice produced
identical SHA-256 hashes.

| Fixture | Bytes | SHA-256 | Contents |
|---|---:|---|---|
| `all-fields.pdf` | 8,172 | `6572c85623d71e4ba64b21417be0b4fad0b939ecffc2790c2e9cd1a6b59fc943` | Text (required, “Ada Lovelace”), checked/unchecked checkbox, 3-option radio (Yearly), list (Beta), dropdown (Singapore), empty signature, empty text |
| `all-fields-flattened.pdf` | 8,886 | `a3a4693e6f775c2e9ba37814caf0169524ab8fa6e27d2f82d85c1cd81113a819` | `flatten_form(all-fields.pdf)` |
| `merged-form.pdf` | 4,481 | `29482c200a5fa3dc0d45015d4210465d05d4c37f060716ceab605767a3ff9234` | Existing `fixtures/pdfs/form-acroform.pdf` plus email text, checked newsletter checkbox and plan dropdown |
| `rotated-90-crop.pdf` | 4,599 | `3d406b850d620bec2d6674da470e40588a9599359ba7b44ae47d630514b8bb84` | `/Rotate 90`, CropBox `[36 36 540 756]`, text/checkbox/dropdown/radio placed from display coordinates |

### Test suite

`cargo test` passes 15 tests:

| File | Tests | Coverage |
|---|---:|---|
| `tests/creation.rs` | 8 | Text AP/DA/DR, static text value pixels, checkbox `Off`/`Yes`, radio `/Kids`+`/AS`, list/combo `/Opt`+`/V`+`/I`, empty signature, merge and name uniquification |
| `tests/fixtures.rs` | 3 | Every field type in the committed fixture, merged values, flattened fixture has no form and renders identically |
| `tests/flatten.rs` | 1 | Form/annots removed, baked appearances drawn, pixel-identical render |
| `tests/geometry.rs` | 2 | Display-to-user-space `/Rect` for all rotations/crops; using raw coordinates on a rotated page misplaces the widget |
| `tests/unicode.rs` | 1 | Non-ASCII `/V` is UTF-16BE with BOM and PDFium reads it back |

### Reader probes

PDFKit (macOS only):

```bash
swift reader/pdfkit_probe.swift fixtures/all-fields.pdf /tmp/pdfkit-all-fields
```

Chrome (uses the installed Chrome binary and Playwright from the main repo):

```bash
NODE_PATH=/Users/nevs/Projects/pdfx/node_modules \
  node reader/chrome_probe.cjs fixtures/all-fields.pdf /tmp/chrome-all-fields.png
NODE_PATH=/Users/nevs/Projects/pdfx/node_modules \
  node reader/chrome_interact.cjs fixtures/all-fields.pdf /tmp/chrome-interaction.png
```

Screenshots are committed under [`reader/evidence/`](../../spikes/acroform-create/reader/evidence/).
The JSON reports produced by `pdfkit_probe.swift` are intentionally not
committed; they contain host-specific paths and are reproducible with the
command above.

## Reader verification

| Reader | Open | Display | Fill/edit | Notes |
|---|---|---|---|---|
| PDFKit (Preview engine) | yes | yes, except rotated AP orientation | not exercised interactively | All field types and values read back; form-free flattened file has 0 annotations; rotated screenshot shows sideways value text |
| Chrome 156.0.8078.48 (headless, installed Chrome) | yes | yes, except rotated AP orientation | yes | Typed into the text field and toggled the checkbox; screenshot evidence |
| Preview.app UI | not verified | not verified | not verified | GUI session locked; PDFKit used as engine-level proxy |
| Adobe Acrobat Reader | not installed | not verified | not verified | No Acrobat binary on this host |

Chrome interaction result: `full_name` changed from `Ada Lovelace` to
`Ada Lovelace-Chrome`, and the checked `subscribe` checkbox toggled off (the
focused empty box is visible in `chrome-interaction.png`).

PDFKit result: `all-fields.pdf` exposes 10 annotations: 2 text, 2 checkbox,
3 radio widgets under `billing`, 1 list (`plan` = Beta), 1 combo (`country` =
Singapore), 1 signature and the empty text field. The merged fixture keeps
`full_name = Ada Lovelace`, `subscribe = off`, and exposes the new `email`,
`newsletter` and `plan = Pro` fields. The flattened fixture has no annotations.

### Rotation result

The widget rectangles land at the requested display positions on a
`/Rotate 90` + CropBox page; the geometry test checks this for all four
rotations. The appearance **content** is not rotation-compensated: field text
is painted sideways, the checkbox check mark is rotated, and the dropdown value
is vertical in both PDFKit and Chrome. See
`reader/evidence/pdfkit-rotated-90-crop.png` and
`reader/evidence/chrome-rotated-90-crop.png`.

This is the main open visual defect. Rect mapping and appearance orientation
are separate problems: fixing `/Rect` does not rotate the AP content.

## Pitfalls found

1. **Every widget needs an explicit `/AP /N` appearance stream.** PDFium and
   the readers do not synthesize appearances for newly created widgets. A
   widget without `/AP` can exist in the object graph and still be invisible.
2. **Reset the fill colour after painting a white background.** The first
   implementation painted the field background white, set only the stroke
   colour to black, then drew the value. The text was white-on-white in every
   static appearance. PDFKit/Chrome interactive views hid the bug by
   regenerating appearances from `/V`; flattening exposed it. The regression
   test now measures dark pixels inside the text field, excluding the border.
3. **Do not set `/NeedAppearances true` to make Chrome show text.** The early
   probe saw blank Chrome text fields with `false`, concluded Chrome required
   `true`, and set it. After fixing the white-fill bug, Chrome and PDFKit show
   and edit the fields correctly with `false` (the default). Generated forms
   therefore omit the flag. This matters for the app: with `true`, PDFium's
   plain page renderer delegates widgets to its form-fill path, which lacks the
   platform callbacks an interactive viewer supplies and paints only partial
   widgets. Merge preserves an existing `/NeedAppearances` value untouched.
4. **`/Rect` is raw user space; display coordinates are not.** For a page with
   CropBox origin `(cx0, cy0)` and displayed size `(cw, ch)`, the raw-to-display
   bottom-left mapping used by `display_rect_to_user_space` is:
   - rotate 0: `u = x - cx0`, `v = y - cy0`
   - rotate 90: `u = y - cy0`, `v = cw - (x - cx0)`
   - rotate 180: `u = cw - (x - cx0)`, `v = ch - (y - cy0)`
   - rotate 270: `u = ch - (y - cy0)`, `v = x - cx0`

   The inverse is implemented and tested. Using raw coordinates directly on a
   rotated page places the widget somewhere else.
5. **Rotated pages also need appearance rotation compensation.** Draw the AP in
   display-local coordinates, then wrap the AP content in a rotation matrix
   (for `/Rotate 90`: `0 1 -1 0 x1 y0 cm`). Without that, field boxes are
   correctly placed but text, check marks and choice values are sideways. The
   spike documents the defect rather than implementing the matrix; ticket 31
   should implement it and add reader screenshots for 90/180/270.
6. **Non-ASCII `/V` is UTF-16BE+BOM, but the appearance font is WinAnsi
   Helvetica.** `你好，PDFX` round-trips through PDFium as a value, but the
   appearance stream cannot draw CJK glyphs with Helvetica/WinAnsi. The spike
   only asserts the value; a production CJK appearance needs the embedded
   subset font pipeline already required by the spec.
7. **An empty signature field has no `/AP`, so viewers draw no visible box.**
   PDFKit lists the annotation and Chrome recognizes the field, but the page
   shows only the surrounding label. If ticket 33 requires a visible signature
   area, generate a blank bordered appearance or `/MK` border.
8. **Merging must preserve the existing AcroForm dictionaries.** Clone the
   catalog/page into `IncrementalDocument`, append to the existing `/Fields`
   array, reuse `/Helv` when present, preserve `/DA`, `/DR` and
   `/NeedAppearances`, and uniquify only new names. Existing field names are
   never renamed.
9. **Flattening depends on the appearance streams.** Widgets without `/AP`
   (unsigned signature) are dropped. If a source form sets
   `/NeedAppearances true`, its file may not contain the appearances the user
   saw, so a flatten can bake stale or empty appearances. The spike's flatten
   is a full save and is intended for export copies, not the incremental edit
   path.
10. **Multiline text appearance layout is not implemented.** The multiline
    flag (`Ff` bit 12) is set, but the appearance draws the value on one line.
    Ticket 31 needs wrapping and line-height layout; this was not fixture-tested.
11. **Radio on-state export values are derived from labels.** Non-alphanumeric
    characters are stripped, empty labels become `ChoiceN`, and collisions get
    a numeric suffix. The parent `/V`, each kid `/AS` and each `/AP` state name
    must all use the same derived value.

## Effort estimate for tickets 31–33

| Ticket | Estimate | Notes |
|---|---:|---|
| 31 — create text fields | 5–7 engineer-days | Shared engine seam, text field/AP, property panel, save integration, undo/redo; multiline layout and CJK appearance add time |
| 32 — create checkbox and radio | 2–3 engineer-days | State machines, radio `/Kids`/`/AS`, option editing, undo/redo |
| 33 — create choice and signature fields | 2–3 engineer-days | List/combo options, multi-select, signature structure and visible-area decision, undo/redo |
| Cross-cutting hardening | 3–5 engineer-days | Rotation-compensated APs, app-side PDFium rendering strategy for existing `/NeedAppearances true` forms, reader regression screenshots, Preview/Acrobat QA |

Total: **12–18 engineer-days (about 2.5–3.5 engineer-weeks)**, within the
spec's 2–4 week estimate. This excludes certificate signing (out of scope) and
does not assume Windows testing on this host.

## Spec/ADR impact

No contradiction with ADR-0004 (incremental save) or ADR-0010 (hybrid engine).
The spike uses a full save only for the flatten export copy, where replacing
the document is acceptable. lopdf creation, PDFium reading/filling, and the
incremental save seam all hold.

The spec should be made explicit about the implementation constraints found.
Proposed exact additions under `### 表单` (after the bullet describing
`/AcroForm` creation around spec line 248):

```markdown
- 新创建的 AcroForm 必须为每个 Widget 写入显式 `/AP` 外观流；生成表单不设置
  `/NeedAppearances`（默认 false），合并已有表单时保留原值。Chrome 与 PDFKit
  在 false 下均可显示和编辑这些字段。
- `/Rect` 使用未旋转的默认用户空间；显示坐标必须按 CropBox 与 `/Rotate` 反算。
  `/Rotate` 为 90/180/270 时，外观流内容还需做旋转补偿，否则字段框位置正确但
  文字/勾选方向错误。
- `/V` 中的非 ASCII 文本使用 UTF-16BE+BOM；外观流中的同类文本需要嵌入 CJK
  子集字体，Helvetica/WinAnsi 只能覆盖拉丁字符。
```

## Not verified / follow-ups

- Preview.app UI and Adobe Acrobat Reader manual open/fill (environment
  limitation; PDFKit/Chrome evidence only).
- Windows behavior.
- Interactive radio and choice editing in Chrome/Preview (state correctness was
  verified through PDFium/PDFKit; only text and checkbox were edited through
  Chrome's viewer).
- Rotation-compensated appearances, multiline appearance layout, and CJK
  appearance fonts (implementation work).
- List multi-select with more than one selected row (code path exists; no
  multi-row fixture).
- Certificate signing and signature validation (explicitly out of scope).
