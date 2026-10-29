import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";

/** Application identity exposed by the Rust command boundary. */
export interface AppInfo {
  name: string;
  version: string;
}

export { DEFAULT_SCALE, MAX_SCALE, MIN_SCALE, ZOOM_STEP } from "./pageGeometry";

/** Stable error codes returned by the Rust command boundary. */
export type CommandErrorCode =
  | "file_not_found"
  | "file_read_failed"
  | "permission_denied"
  | "pdfium_unavailable"
  | "password_required"
  | "invalid_password"
  | "corrupt_document"
  | "document_not_found"
  | "page_out_of_range"
  | "invalid_scale"
  | "invalid_argument"
  | "no_editable_text"
  | "unsupported_font"
  | "unsupported_image_format"
  | "image_decode_failed"
  | "image_too_large"
  | "render_failed"
  | "file_write_failed"
  | "export_target_is_source"
  | "export_target_is_output"
  | "flatten_export_failed"
  | "print_failed"
  | "window_failed"
  | "save_failed"
  | "save_target_is_source"
  | "no_output_document"
  | "unencrypted_output_confirmation_required"
  | "encrypted_output_confirmation_required"
  | "signature_invalidation_confirmation_required"
  | "read_only_document"
  | "layer_not_found"
  | "layer_name_invalid"
  | "layer_delete_unsupported"
  | "attachment_too_large"
  | "attachment_not_found"
  | "attachment_open_confirmation_required"
  | "attachment_open_failed"
  | "attachment_target_is_source"
  | "attachment_target_is_output"
  | "attachment_save_failed"
  | "recovery_failed"
  | "recovery_not_found"
  | "form_field_not_found"
  | "form_field_read_only"
  | "form_fill_not_permitted"
  | "form_value_invalid"
  | "form_field_name_invalid"
  | "form_field_name_duplicate"
  | "unsupported_link"
  | "update_not_configured"
  | "update_check_failed"
  | "update_manifest_invalid"
  | "update_open_failed"
  | "internal";

/** A structured error crossing the Tauri command boundary. */
export interface CommandError {
  code: CommandErrorCode;
  message: string;
}

/** Editing capabilities reported by the PDF security handler. */
export interface DocumentPermissions {
  canModifyContent: boolean;
  canAnnotate: boolean;
  canAssemble: boolean;
  canFillForms: boolean;
  canPrint: boolean;
  canCopyContent: boolean;
}

/**
 * Counts of the digital-signature structures in a document.
 *
 * The backend deliberately exposes counts only: signer identities, signing
 * reasons, document text and signature bytes never cross the command boundary.
 */
export interface SignatureSummary {
  /** Signature dictionaries (`/V`); non-zero means an actual signature exists. */
  signedCount: number;
  /** `/FT /Sig` fields, including empty fields that were never signed. */
  fieldCount: number;
}

export interface DocumentView {
  documentId: string;
  name: string;
  path: string;
  pageCount: number;
  pageWidth: number;
  pageHeight: number;
  /** The Source Document is encrypted (User or Owner Password). */
  encrypted: boolean;
  /** Permission Restrictions deny every editing capability. */
  readOnly: boolean;
  permissions: DocumentPermissions;
  /**
   * Output Document this Document will save to, when one exists. A document
   * restored from a Recovery File reuses the previous Output Document only
   * while it still exists and is not the Source Document.
   */
  outputPath?: string | null;
  /**
   * The Output Document at `outputPath` was written with an opening password.
   * Save Changes must ask before replacing it with an unencrypted file; the
   * password itself is never persisted or returned by the backend.
   */
  outputEncrypted?: boolean;
  /** Opened from a Recovery File; `path` is the original Source Document. */
  recovered?: boolean;
  /**
   * Signature structures detected when the document was opened, or null for an
   * ordinary document. Editing or saving invalidates actual signatures; PDFX
   * never signs and never modifies these structures.
   */
  signature?: SignatureSummary | null;
}

export interface PageImageView {
  documentId: string;
  pageIndex: number;
  scale: number;
  width: number;
  height: number;
  pngBase64: string;
}

/**
 * Display metadata for one page. `width` and `height` already include the
 * page's intrinsic rotation; `rotation` and the CropBox origin let overlays
 * map display coordinates back to PDF user space.
 */
export interface PageInfo {
  pageIndex: number;
  width: number;
  height: number;
  rotation: number;
  cropX: number;
  cropY: number;
}

/**
 * A viewport-sized slice of a page rendered at high zoom. `x` and `y` are
 * offsets in display pixels at `scale`, measured from the page's top-left
 * corner.
 */
export interface PageRegionView {
  documentId: string;
  pageIndex: number;
  scale: number;
  x: number;
  y: number;
  width: number;
  height: number;
  pngBase64: string;
}

export interface ExtractedTextView {
  pageIndex: number;
  text: string;
  charCount: number;
}

/**
 * One positioned character of a page's text layer.
 *
 * All geometry is in display-space points: the top-left origin of the page as
 * rendered, with the page's intrinsic rotation and CropBox origin already
 * applied. Divide by `pageWidth`/`pageHeight` to position an overlay.
 * Generated characters are PDFium-inserted line breaks and have zero-size
 * boxes; they are useful for copy text but are not hit-testable.
 */
export interface TextCharView {
  text: string;
  x: number;
  y: number;
  width: number;
  height: number;
  originX: number;
  originY: number;
  fontSize: number;
  generated: boolean;
}

/** The text layer of one page, including pages with no editable text. */
export interface TextLayerView {
  pageIndex: number;
  pageWidth: number;
  pageHeight: number;
  rotation: number;
  cropX: number;
  cropY: number;
  hasText: boolean;
  text: string;
  chars: TextCharView[];
}

/**
 * A half-open character range inside a Text Box.
 *
 * Offsets count Unicode code points, not UTF-16 code units. The textarea
 * reports UTF-16 offsets, so the canvas converts before calling the command.
 */
export interface TextRangeView {
  start: number;
  end: number;
}

/**
 * One styled range inside a Text Box.
 *
 * The style values are the effective ones after font resolution: a range that
 * could not keep its requested family is reported with the bundled family it
 * actually renders with. `alignment` stays a box-level property.
 */
export interface TextBoxRunView {
  start: number;
  end: number;
  fontFamily: string;
  fontSize: number;
  bold: boolean;
  italic: boolean;
  underline: boolean;
  /** Fill colour as `#rrggbb`. */
  color: string;
}

/**
 * Initial state of a Text Box when the user clicks existing text.
 *
 * Content and style come from the original Text Object; geometry is in
 * display-space points, the same coordinate system as the text layer.
 */
export interface TextBoxTargetView {
  boxId: number;
  pageIndex: number;
  text: string;
  fontFamily: string;
  fontSize: number;
  /** Fill colour as `#rrggbb`. */
  color: string;
  x: number;
  y: number;
  width: number;
  height: number;
  /** Style variant flags; an original Text Object's font name can imply them. */
  bold: boolean;
  italic: boolean;
  underline: boolean;
  alignment: TextAlignment;
  /** Clockwise rotation in degrees in display space. */
  rotation: number;
  /** The height follows the content and never shrinks automatically. */
  autoHeight: boolean;
  /** Created by the Text Box tool rather than being an original Text Object. */
  created: boolean;
  /**
   * Styled character ranges, sorted and non-overlapping.
   *
   * The base style fields above apply to every character outside these runs;
   * an empty array means the whole box uses the base style.
   */
  runs: TextBoxRunView[];
}

/** Horizontal alignment of the lines inside a Text Box. */
export type TextAlignment = "left" | "center" | "right";

/** Whole-box style the Text Box editor sends to the backend. */
export interface TextBoxStyleView {
  fontFamily: string;
  fontSize: number;
  bold: boolean;
  italic: boolean;
  underline: boolean;
  /** Fill colour as `#rrggbb`. */
  color: string;
  alignment: TextAlignment;
}

/**
 * One font the Text Box editor may use.
 *
 * `source` distinguishes PDF base-14 fonts from embedded font programs;
 * `embedded` says whether a program is written into the saved PDF and
 * `coverage` states which characters the font can render. The registry lists
 * the base-14 families plus the bundled Noto Sans SC CJK family; embedded
 * source fonts in an opened document are reused without appearing here.
 */
export interface TextBoxFontView {
  family: string;
  label: string;
  bold: boolean;
  italic: boolean;
  postscriptName: string;
  source: "base14" | "embedded" | string;
  embedded: boolean;
  embeddable: boolean;
  coverage: "win_ansi" | "cjk" | "full" | string;
}

/** Geometry for a new Text Box, in display-space points. */
export interface CreateTextBoxRequest {
  x: number;
  y: number;
  width: number;
  height: number;
  /** Omit for the default visible style (Helvetica 16, black, left). */
  style?: TextBoxStyleView | null;
}

/** Move, resize or rotate an existing Text Box. */
export interface TextBoxTransformRequest {
  x: number;
  y: number;
  width: number;
  height: number;
  /** False after a manual resize: the height then clips instead of growing. */
  autoHeight: boolean;
  /** Clockwise rotation in degrees in display space. */
  rotation: number;
}

/** The outcome of creating or restyling a Text Box. */
export interface TextBoxEditResultView {
  pageIndex: number;
  target: TextBoxTargetView;
  /** Set when an existing font could not be preserved. */
  font: FontUsageView | null;
  history: EditHistoryView;
}

/** The font chosen for a committed edit, when it differs from the original. */
export interface FontUsageView {
  originalFamily: string;
  family: string;
  fallback: boolean;
  /** Why the original font could not be kept, when `fallback` is true. */
  reason: string | null;
}

/**
 * Document-wide undo/redo state shared by every edit tool.
 *
 * `revision` is the current document state; `savedRevision` is the state last
 * written to disk. Undoing back to the saved state clears `dirty`. History
 * survives saving.
 */
export interface EditHistoryView {
  canUndo: boolean;
  canRedo: boolean;
  revision: number;
  savedRevision: number;
  dirty: boolean;
}

/** The outcome of committing a text edit or deletion. */
export interface TextEditResultView {
  pageIndex: number;
  boxId: number;
  /** Text Box content after the edit (empty after a deletion). */
  text: string;
  /** Set when the replacement could not keep the original font. */
  font: FontUsageView | null;
  history: EditHistoryView;
}

/** The outcome of an undo or redo step. */
export interface EditStepView {
  /** Page whose content changed, so only that page needs refreshing. */
  pageIndex: number;
  /** Visible page count after the step. */
  pageCount: number;
  /**
   * True when the page order, count or rotation changed; caches keyed by
   * page index (page metadata, thumbnails) must be refreshed.
   */
  pageStructureChanged: boolean;
  history: EditHistoryView;
}

/** Paper size of an inserted blank page, in PDF points. */
export interface BlankPageSize {
  width: number;
  height: number;
}

/** The outcome of a rotate, delete, move or blank-page insertion. */
export interface PageOperationView {
  /** Visible page count after the operation. */
  pageCount: number;
  /** Logical page the viewer should select next, clamped to range. */
  pageIndex: number;
  history: EditHistoryView;
}

/**
 * One image placed on a Page.
 *
 * Geometry is in display-space points (top-left origin, CropBox and intrinsic
 * rotation already applied), exactly like Text Box geometry. `rotation` is
 * clockwise around the rectangle's center. The rectangle may extend beyond
 * the page; the page clips it when rendering and saving.
 */
export interface ImageObjectView {
  imageId: number;
  pageIndex: number;
  /** Label shown on the overlay: a file name or the signature source. */
  name: string;
  /**
   * A photo or a signature. Signatures are the same page image objects, with
   * their own label and smaller default size, so the overlay can badge them.
   */
  kind: ImageKind;
  x: number;
  y: number;
  width: number;
  height: number;
  /** Clockwise rotation in degrees in display space. */
  rotation: number;
  pixelWidth: number;
  pixelHeight: number;
}

/** What a page image represents. */
export type ImageKind = "image" | "signature";

/** Move, resize or rotate an image around its center. */
export interface ImageTransformRequest {
  x: number;
  y: number;
  width: number;
  height: number;
  /** Clockwise rotation in degrees in display space. */
  rotation: number;
}

/** The outcome of inserting or transforming an image. */
export interface ImageEditResultView {
  pageIndex: number;
  image: ImageObjectView;
  history: EditHistoryView;
}

/** Which logical pages receive a watermark. */
export type WatermarkScope = "current" | "all";

/** A text or image watermark. */
export type WatermarkKind = "text" | "image";

/**
 * One watermark apply request. Opacity is a percentage (1-100), rotation is
 * clockwise degrees around the mark center, scale is a percentage of the
 * default size, and positions are fractions of the display page (0-1).
 */
export interface WatermarkRequest {
  scope: WatermarkScope;
  kind: WatermarkKind;
  text: string | null;
  imagePath: string | null;
  opacity: number;
  rotation: number;
  scale: number;
  positionX: number;
  positionY: number;
}

/** The active watermark as the panel sees it. */
export interface WatermarkView {
  kind: WatermarkKind;
  /** The scope chosen when this watermark was applied. */
  scope: WatermarkScope;
  text: string | null;
  imageName: string | null;
  imagePath: string | null;
  opacity: number;
  rotation: number;
  scale: number;
  positionX: number;
  positionY: number;
  /** Logical pages carrying the mark, in page order. */
  pageIndices: number[];
}

/** The outcome of applying or deleting the watermark. */
export interface WatermarkEditResult {
  watermark: WatermarkView | null;
  history: EditHistoryView;
}

/** A local image validated for the live image-watermark preview. */
export interface WatermarkImagePreview {
  name: string;
  mimeType: string;
  dataUrl: string;
  width: number;
  height: number;
}

/**
 * One optional-content layer (OCG) in the open document.
 *
 * `index` is the layer's position in `/OCProperties /OCGs`; it is stable for
 * the session and is the identity commands take back. Layers are
 * document-wide: a layer may only paint on some pages, but the panel lists
 * every OCG in the document, like desktop PDF editors do.
 */
export interface LayerView {
  index: number;
  name: string;
  visible: boolean;
}

/** The document's layer list; `documentId` discards stale responses. */
export interface LayerListView {
  documentId: string;
  layers: LayerView[];
}

/** The outcome of a visibility, rename or delete action. */
export interface LayerEditResultView {
  layers: LayerView[];
  history: EditHistoryView;
}

/**
 * The two linear page-content objects PDFX creates.
 *
 * Lines and arrows are path objects written into the page content stream, not
 * annotations. They therefore never appear in [`AnnotationView`] or the
 * annotation list, and they have no annotation thread or standard annotation
 * delete semantics; deleting one is an undoable content edit.
 */
export type LineKind = "line" | "arrow";

/** Endpoints in display-space points (top-left origin). */
export interface LineGeometry {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
}

/**
 * Stroke colour, opacity and width of a line or arrow. Lines always have a
 * stroke, so there is no fill control. Opacity is a percentage (0-100) applied
 * to the stroke (and to the arrow head's fill). Line width is in page points.
 */
export interface LineStyle {
  strokeColor: string;
  opacity: number;
  lineWidth: number;
}

/** Draws a line or arrow on one page. */
export interface CreateLineRequest {
  pageIndex: number;
  kind: LineKind;
  geometry: LineGeometry;
  style: LineStyle;
}

/** New endpoints for an existing line; rotation is expressed by the endpoints. */
export interface LineTransformRequest {
  geometry: LineGeometry;
}

/** A line or arrow this editing session created on a page. */
export interface LineObjectView {
  lineId: number;
  pageIndex: number;
  kind: LineKind;
  geometry: LineGeometry;
  style: LineStyle;
}

/** The outcome of creating or changing one linear object. */
export interface LineEditResultView {
  pageIndex: number;
  line: LineObjectView;
  history: EditHistoryView;
}

/** Widget types PDFX can display from an existing AcroForm. */
export type FormFieldKind =
  | "text"
  | "checkbox"
  | "radio"
  | "comboBox"
  | "listBox"
  | "pushButton"
  | "signature"
  | "unknown";

/** A widget rectangle in display-space points (top-left origin). */
export interface FormFieldRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * One interactive widget of the open document.
 *
 * A radio group has one view per widget; the widgets share `name` and
 * `radioGroup`, `value` is the selected export state of the group and
 * `checked` tells whether this particular widget is the selected one.
 */
export interface FormFieldView {
  /** Stable within an open document: `"<page-index>:<annotation-index>"`. */
  fieldId: string;
  name: string;
  kind: FormFieldKind;
  pageIndex: number;
  /** Zero-based position in document-wide tab order. */
  order: number;
  rect: FormFieldRect;
  /** Text value, radio-group value or selected choice label. */
  value: string | null;
  /** Checkbox/radio state; always false for other kinds. */
  checked: boolean;
  /** Choice labels in document order; empty for other kinds. */
  options: string[];
  selectedIndex: number | null;
  /** Every selected option; only a multi-select list box has several. */
  selectedIndices: number[];
  /** True for a list box whose MultiSelect flag is set. */
  multiSelect: boolean;
  required: boolean;
  readOnly: boolean;
  multiline: boolean;
  /** The shared field name for radio widgets. */
  radioGroup: string | null;
  /** The widget's export value for radio widgets. */
  exportValue: string | null;
  /** `/TU` tooltip, when the field has one. */
  tooltip: string | null;
  /** Font size from the widget's `/DA`, when it can be read. */
  fontSize: number | null;
  /** True for a text field created in this session. */
  created: boolean;
}

/** Places a new text Form Field on a page. */
export interface CreateFormTextFieldRequest {
  /** Display-space rectangle, top-left origin. */
  rect: FormFieldRect;
  /** `null` asks the backend for a generated `text_field_N` name. */
  name: string | null;
  tooltip: string | null;
  required: boolean;
  multiline: boolean;
  fontSize: number | null;
}

/** The new field id, the refreshed widget list and the shared history. */
export interface CreateFormTextFieldResult {
  fieldId: string;
  fields: FormFieldView[];
  history: EditHistoryView;
}

/** The full desired state of a created field. */
export interface UpdateFormTextFieldRequest {
  name: string;
  tooltip: string | null;
  required: boolean;
  multiline: boolean;
  fontSize: number;
}

/** The refreshed widget list and shared history after a property change. */
export interface FormStructureEditResult {
  fields: FormFieldView[];
  history: EditHistoryView;
}

/** Places a text, checkbox, radio, choice or signature field on a page. */
export type CreateFormFieldRequest =
  | ({ kind: "text" } & CreateFormTextFieldRequest)
  | {
      kind: "checkbox";
      /** Display-space rectangle, top-left origin. */
      rect: FormFieldRect;
      /** `null` asks the backend for a generated `checkbox_N` name. */
      name: string | null;
      tooltip: string | null;
      required: boolean;
      /** Initial `/V` and `/AS` state. */
      checked: boolean;
    }
  | {
      kind: "radio";
      /** The first option's rectangle; later options stack below it. */
      rect: FormFieldRect;
      /** `null` asks the backend for a generated `radio_group_N` name. */
      name: string | null;
      tooltip: string | null;
      required: boolean;
      /** Option labels in display order. */
      options: string[];
      /** Index of the initially selected option, or `null` for none. */
      selectedIndex: number | null;
    }
  | {
      kind: "comboBox";
      /** Display-space rectangle, top-left origin. */
      rect: FormFieldRect;
      /** `null` asks the backend for a generated `combo_box_N` name. */
      name: string | null;
      tooltip: string | null;
      required: boolean;
      /** `/Opt` labels in display order. An empty list creates one option. */
      options: string[];
      /** Default selected option index, or `null` for none. */
      selectedIndex: number | null;
      fontSize: number | null;
    }
  | {
      kind: "listBox";
      /** Display-space rectangle, top-left origin. */
      rect: FormFieldRect;
      /** `null` asks the backend for a generated `list_box_N` name. */
      name: string | null;
      tooltip: string | null;
      required: boolean;
      /** `/Opt` labels in display order. An empty list creates one option. */
      options: string[];
      /** Default selected option indices; a single-select list takes one. */
      selectedIndices: number[];
      /** Sets the MultiSelect flag (list boxes only). */
      multiSelect: boolean;
      fontSize: number | null;
    }
  | {
      /** An empty signature placeholder: structure only, never a signature. */
      kind: "signature";
      /** Display-space rectangle, top-left origin. */
      rect: FormFieldRect;
      /** `null` asks the backend for a generated `signature_N` name. */
      name: string | null;
      tooltip: string | null;
    };

/** The selected widget id, the refreshed widget list and shared history. */
export interface CreateFormFieldResult {
  fieldId: string;
  fields: FormFieldView[];
  history: EditHistoryView;
}

/** The full desired state of any created field. */
export type UpdateFormFieldRequest =
  | ({ kind: "text" } & UpdateFormTextFieldRequest)
  | {
      kind: "checkbox";
      name: string;
      tooltip: string | null;
      required: boolean;
      checked: boolean;
    }
  | {
      kind: "radio";
      name: string;
      tooltip: string | null;
      required: boolean;
      /** The complete option-label list in display order. */
      options: string[];
      selectedIndex: number | null;
    }
  | {
      kind: "comboBox";
      name: string;
      tooltip: string | null;
      required: boolean;
      options: string[];
      selectedIndex: number | null;
      fontSize: number;
    }
  | {
      kind: "listBox";
      name: string;
      tooltip: string | null;
      required: boolean;
      options: string[];
      selectedIndices: number[];
      multiSelect: boolean;
      fontSize: number;
    }
  | {
      /** Signature fields carry only identity: PDFX never signs them. */
      kind: "signature";
      name: string;
      tooltip: string | null;
    };

/** A value the user committed for one widget. */
export type FormFieldValueInput =
  | { kind: "text"; text: string }
  | { kind: "checked"; checked: boolean }
  | { kind: "choice"; value: string };

/** The outcome of one field change: all fields refreshed, plus history. */
export interface FormFieldEditResultView {
  fieldId: string;
  fields: FormFieldView[];
  history: EditHistoryView;
}

/** The text markup annotation kinds PDFX creates. */
export type TextMarkupKind = "highlight" | "underline" | "strikeout" | "squiggly";

/** The geometric shape kinds PDFX creates. */
export type ShapeKind = "square" | "circle";

/**
 * The two free text layouts. Both are native `/FreeText` annotations; a
 * callout additionally paints a bordered, tinted box. Neither is a comment
 * thread.
 */
export type FreeTextKind = "freetext" | "callout";

/** Horizontal alignment of free text inside its box (`/Q`). */
export type FreeTextAlign = "left" | "center" | "right";

/** Every annotation kind the list, overlays and style panel manage. */
export type AnnotationKind =
  | TextMarkupKind
  | ShapeKind
  | FreeTextKind
  | "note"
  | "ink"
  | "stamp";

/** A display-space rectangle of a markup selection, note icon or ink bounds. */
export interface TextMarkupRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** One freehand point in display-space points. */
export interface InkPoint {
  x: number;
  y: number;
}

/** Creates a native text markup annotation over the selected rectangles. */
export interface CreateTextMarkupRequest {
  pageIndex: number;
  kind: TextMarkupKind;
  /** Fill or stroke colour as `#rrggbb`. */
  color: string;
  /** Constant opacity in percent (0-100). */
  opacity: number;
  /** Display-space rectangles; one per selected line. */
  rects: TextMarkupRect[];
}

/** The four built-in stamps. */
export type StampKind = "approved" | "draft" | "confidential" | "rejected";

/**
 * A stamp frame in display-space page points: top-left origin and clockwise
 * rotation in degrees, the same convention as the Text Box overlay.
 */
export interface StampFrameView {
  x: number;
  y: number;
  width: number;
  height: number;
  rotation: number;
}

/** The built-in design and editable frame of a stamp annotation. */
export interface StampView {
  kind: StampKind;
  frame: StampFrameView;
}

/** Places one built-in stamp with the given opacity percentage. */
export interface CreateStampRequest {
  pageIndex: number;
  kind: StampKind;
  /** Constant opacity in percent (0-100); the ink colour comes from the kind. */
  opacity: number;
  /** Display-space frame. */
  frame: StampFrameView;
}

/** The editable body and typography of a free text or callout annotation. */
export interface FreeTextView {
  kind: FreeTextKind;
  /** Font size in page points. */
  fontSize: number;
  align: FreeTextAlign;
}

/** Places a native free text box or callout box in display space. */
export interface CreateFreeTextRequest {
  pageIndex: number;
  kind: FreeTextKind;
  /** Display-space box; text wraps inside it. */
  rect: TextMarkupRect;
  /** Text and callout border colour as `#rrggbb`. */
  color: string;
  /** Constant opacity in percent (0-100). */
  opacity: number;
  text: string;
  fontSize: number;
  align: FreeTextAlign;
}

/** Replaces a free text body, alignment, size, colour and opacity at once. */
export interface UpdateFreeTextRequest {
  name: string;
  text: string;
  fontSize: number;
  align: FreeTextAlign;
  color: string;
  opacity: number;
}

/** Moves, resizes or rotates a stamp as one undoable action. */
export interface TransformStampRequest {
  name: string;
  frame: StampFrameView;
}

/** Places a native sticky note with its icon box in display space. */
export interface CreateNoteRequest {
  pageIndex: number;
  /** Icon colour as `#rrggbb`. */
  color: string;
  /** Constant opacity in percent (0-100). */
  opacity: number;
  /** Note body; edited later through `updateNoteText`. */
  text: string;
  rect: TextMarkupRect;
}

/** Draws one native freehand ink annotation. */
export interface CreateInkRequest {
  pageIndex: number;
  color: string;
  /** Constant opacity in percent (0-100). */
  opacity: number;
  /** Stroke width in display points. */
  lineWidth: number;
  /** Display-space strokes; one entry per pen-down/pen-up gesture. */
  strokes: InkPoint[][];
}

/**
 * A native annotation read back from the in-memory document.
 *
 * `name` is the stable `/NM` identity used by locate, restyle, transform and
 * delete. `author` and `createdAt` are copied into the PDF at creation time;
 * `createdAt` is the PDF date string (`D:YYYYMMDDHHmmSSZ`).
 */
export interface AnnotationView {
  name: string;
  pageIndex: number;
  kind: AnnotationKind;
  author: string | null;
  createdAt: string | null;
  color: string;
  opacity: number;
  /** Ink stroke width in display points; null for other kinds. */
  lineWidth: number | null;
  /** Sticky-note body; null for other kinds. */
  text: string | null;
  /** Display-space boxes: one per markup quad, or the stamp's display bounds. */
  rects: TextMarkupRect[];
  /** Display-space ink strokes; empty for other kinds. */
  strokes: InkPoint[][];
  /** Shape geometry and style; null for every other kind. */
  shape: ShapeView | null;
  /** Set only for stamp annotations. */
  stamp: StampView | null;
  /** Set only for free text and callout annotations. */
  freeText: FreeTextView | null;
}

/** A shape's unrotated display-space geometry plus clockwise rotation. */
export interface ShapeGeometry {
  x: number;
  y: number;
  width: number;
  height: number;
  /** Clockwise degrees around the rectangle's centre (0-360). */
  rotation: number;
}

/**
 * Fill, stroke, opacity and line width of a shape. A null colour means the
 * shape has no fill or no stroke; at least one must be present. Opacity is a
 * percentage (0-100) applied to the whole annotation. Line width is in page
 * points and is ignored without a stroke.
 */
export interface ShapeStyle {
  fillColor: string | null;
  strokeColor: string | null;
  opacity: number;
  lineWidth: number;
}

/** A shape annotation as the canvas overlay and property panel see it. */
export interface ShapeView {
  kind: ShapeKind;
  geometry: ShapeGeometry;
  style: ShapeStyle;
}

/** Draws a rectangle or ellipse on one page. */
export interface CreateShapeRequest {
  pageIndex: number;
  kind: ShapeKind;
  geometry: ShapeGeometry;
  style: ShapeStyle;
}

/** The outcome of creating, restyling or deleting one annotation. */
export interface AnnotationMutationView {
  /** Null after a deletion. */
  annotation: AnnotationView | null;
  pageIndex: number;
  history: EditHistoryView;
}

/** A native link's target: an external URL or another page in this document. */
export type LinkTarget =
  | { kind: "url"; url: string }
  | {
      kind: "page";
      /** Zero-based page index in the same document. */
      pageIndex: number;
    };

/** Why an existing link's target cannot be edited without corrupting it. */
export type LinkUnsupportedReason = "action" | "destination" | "malformed";

/**
 * One native `/Link` annotation as the canvas overlay and link dialog see it.
 *
 * `id` is the link's position on its page (`"page:index"`). Every mutation
 * reloads the list because creating or editing a link appends a new native
 * annotation. `target` is present exactly when the link is editable;
 * `unsupported` is present exactly when PDFX refuses to touch it.
 */
export interface LinkView {
  id: string;
  /** Stable `/NM` identity; absent on links written by other tools. */
  name: string | null;
  pageIndex: number;
  target: LinkTarget | null;
  unsupported: LinkUnsupportedReason | null;
  /** Display-space boxes: one per `/QuadPoints` entry, or `/Rect`. */
  rects: TextMarkupRect[];
}

/** Adds one link over a text selection's quads or an image's bounds. */
export interface CreateLinkRequest {
  pageIndex: number;
  /** Display-space boxes; one per selected text line, or one per image. */
  rects: TextMarkupRect[];
  target: LinkTarget;
}

/** Replaces one existing link's target as a single undoable action. */
export interface UpdateLinkTargetRequest {
  /** The positional id from {@link LinkView}. */
  linkId: string;
  target: LinkTarget;
}

/** The outcome of creating, editing or deleting one link. */
export interface LinkMutationView {
  /** Null after a deletion. */
  link: LinkView | null;
  pageIndex: number;
  history: EditHistoryView;
}

/** The two save entry points. */
export type SaveKind = "save_as" | "save_changes";

/** Stages of one save, emitted in order; `done` is last. */
export type SaveStage =
  | "preparing"
  | "serializing"
  | "finalizing"
  | "writing"
  | "done";

/** One progress update from the save command. */
export interface SaveProgress {
  documentId: string;
  stage: SaveStage;
  completed: number;
  total: number;
}

/**
 * A save request. `saveAsPath` is the chosen Output Document path for Save As
 * and null for Save Changes to the remembered output.
 */
export interface SaveDocumentRequest {
  saveAsPath: string | null;
  /** True only after the user accepted the unencrypted-output warning. */
  confirmUnencryptedOutput: boolean;
  /**
   * True only after the user accepted that saving a document containing
   * signature structures invalidates those signatures. Omitted when no
   * confirmation was asked; the backend refuses the write in that case.
   */
  confirmSignatureInvalidation?: boolean;
  /**
   * Opening password the Output Document must require. Absent means the
   * unencrypted ADR-0007 default. It is held only for the in-flight request:
   * callers must not persist it in settings, logs or save-error state.
   */
  password?: string;
}

/** The outcome of one completed save. */
export interface SaveResultView {
  documentId: string;
  outputPath: string;
  kind: SaveKind;
  bytesWritten: number;
  /** True when the written Output Document requires an opening password. */
  encrypted: boolean;
  /** The previous Output Document, preserved before it was replaced. */
  backupPath: string | null;
  /** Signature structures still present after the save; counts only. */
  signature?: SignatureSummary | null;
  /**
   * The save invalidated an actual signature (`signedCount > 0` before and
   * after the write); the warning banner switches to its invalidated state.
   */
  signatureInvalidated?: boolean;
  /** Clean state after the write; undo/redo history is untouched. */
  history: EditHistoryView;
}

/**
 * Metadata for one Recovery File.
 *
 * The sidecar never contains document text or passwords: only provenance,
 * revision counters and a timestamp. `sourceEncrypted` warns that the snapshot
 * is an unencrypted local copy of an encrypted Source Document (ADR-0007).
 */
export interface RecoverySnapshotView {
  formatVersion: number;
  sessionId: string;
  /** Original Source Document path; never written when restoring. */
  sourcePath: string;
  outputPath: string | null;
  displayName: string;
  sourceEncrypted: boolean;
  sourceReadOnly: boolean;
  sourcePermissions: DocumentPermissions | null;
  dirty: boolean;
  revision: number;
  savedRevision: number;
  createdAtMillis: number;
  appVersion: string;
}

/** The result of one throttled Recovery File tick. */
export interface RecoverySnapshotOutcome {
  written: boolean;
  sessionId: string | null;
  bytes: number;
}

/** One entry of the locally persisted recent-files list. */
export interface RecentFileView {
  path: string;
  name: string;
  lastOpenedMillis: number;
}

/** Tauri event carrying save progress for the window that started the save. */
export const SAVE_PROGRESS_EVENT = "pdfx://save-progress";

/** Event emitted to the focused window for native menu activation. */
export const MENU_EVENT = "pdfx://menu";

const COMMAND_ERROR_CODES: ReadonlySet<string> = new Set<CommandErrorCode>([
  "file_not_found",
  "file_read_failed",
  "permission_denied",
  "pdfium_unavailable",
  "password_required",
  "invalid_password",
  "corrupt_document",
  "document_not_found",
  "page_out_of_range",
  "invalid_scale",
  "invalid_argument",
  "no_editable_text",
  "unsupported_font",
  "unsupported_image_format",
  "image_decode_failed",
  "image_too_large",
  "render_failed",
  "file_write_failed",
  "export_target_is_source",
  "export_target_is_output",
  "flatten_export_failed",
  "print_failed",
  "window_failed",
  "save_failed",
  "save_target_is_source",
  "no_output_document",
  "unencrypted_output_confirmation_required",
  "signature_invalidation_confirmation_required",
  "read_only_document",
  "layer_not_found",
  "layer_name_invalid",
  "layer_delete_unsupported",
  "attachment_too_large",
  "attachment_not_found",
  "attachment_open_confirmation_required",
  "attachment_open_failed",
  "attachment_target_is_source",
  "attachment_target_is_output",
  "attachment_save_failed",
  "recovery_failed",
  "recovery_not_found",
  "form_field_not_found",
  "form_field_read_only",
  "form_fill_not_permitted",
  "form_value_invalid",
  "form_field_name_invalid",
  "form_field_name_duplicate",
  "unsupported_link",
  "internal",
]);

/**
 * Reads the application identity through the single Tauri command seam.
 *
 * Every frontend feature talks to the Rust backend through this module, so
 * tests can replace the IPC boundary without knowing about PDF internals.
 */
export async function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>("app_info");
}

/** Opens a native file dialog filtered to PDFs. Returns null when cancelled. */
export async function choosePdfFile(): Promise<string | null> {
  const selection = await openDialog({
    multiple: false,
    directory: false,
    filters: [{ name: "PDF", extensions: ["pdf"] }],
  });
  return typeof selection === "string" ? selection : null;
}

/**
 * Opens a native file dialog filtered to the image formats the backend
 * accepts. Returns null when cancelled.
 */
export async function chooseImageFile(): Promise<string | null> {
  const selection = await openDialog({
    multiple: false,
    directory: false,
    filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg"] }],
  });
  return typeof selection === "string" ? selection : null;
}

/**
 * Opens a native file dialog for an attachment. Attachments may be any file,
 * so no extension filter is applied. Returns null when cancelled.
 */
export async function chooseAttachmentFile(): Promise<string | null> {
  const selection = await openDialog({
    multiple: false,
    directory: false,
    title: "添加附件",
  });
  return typeof selection === "string" ? selection : null;
}

/**
 * Opens the native save dialog for extracting an attachment. Returns null
 * when cancelled.
 */
export async function chooseAttachmentDestination(
  defaultName: string,
): Promise<string | null> {
  const selection = await saveDialog({
    defaultPath: defaultName,
    title: "另存附件",
  });
  return typeof selection === "string" ? selection : null;
}

/** Image format of an exported Page. */
export type PageImageFormat = "png" | "jpg";
/** Confirmation metadata for a written page image. */
export interface PageExportView {
  path: string;
  format: PageImageFormat;
  dpi: number;
  width: number;
  height: number;
  bytes: number;
}

/**
 * Writes one Page as a PNG or JPEG image at `dpi` (72 = actual size).
 * Export is read-only: it never changes the Document or its unsaved state.
 */
export async function exportPage(
  documentId: string,
  pageIndex: number,
  format: PageImageFormat,
  dpi: number,
  destination: string,
): Promise<PageExportView> {
  return invoke<PageExportView>("export_page", {
    documentId,
    pageIndex,
    format,
    dpi,
    destination,
  });
}

/**
 * Opens the platform print dialog for the print-only surface the caller built.
 * The backend re-checks the Document's Permission Restrictions first.
 */
export async function printDocument(documentId: string): Promise<void> {
  return invoke<void>("print_document", { documentId });
}

/** Native save dialog for a page image. Returns null when cancelled. */
export async function choosePageImageDestination(
  defaultPath: string,
  format: PageImageFormat,
): Promise<string | null> {
  const selection = await saveDialog({
    defaultPath,
    filters: [
      {
        name: format === "png" ? "PNG" : "JPG",
        extensions: [format === "png" ? "png" : "jpg"],
      },
    ],
  });
  return typeof selection === "string" ? selection : null;
}

/** Why an annotation was omitted from a flattened copy. */
export type FlattenSkipReason =
  | "popup"
  | "no_appearance"
  | "unsupported_appearance"
  | "hidden";

/** One annotation removed from a flattened copy without being drawn. */
export interface FlattenSkipView {
  pageIndex: number;
  /** The annotation's `/Subtype`, without the slash. */
  subtype: string;
  reason: FlattenSkipReason;
}

/** Confirmation metadata for a written flattened copy. */
export interface FlattenExportView {
  documentId: string;
  path: string;
  bytes: number;
  /** Visual annotations drawn into page content. */
  flattened: number;
  /** Links and form widgets left interactive in the copy. */
  preserved: number;
  skipped: FlattenSkipView[];
}

/**
 * Writes a new PDF whose visual annotations are page content while links,
 * form fields and signature widgets stay interactive. The live Document, its
 * unsaved state and the Source Document are never modified.
 *
 * An encrypted Source Document produces an unencrypted copy only when
 * `confirmUnencryptedOutput` is true (ADR-0007).
 */
export async function exportFlattenedCopy(
  documentId: string,
  destination: string,
  confirmUnencryptedOutput = false,
): Promise<FlattenExportView> {
  return invoke<FlattenExportView>("export_flattened_copy", {
    documentId,
    destination,
    confirmUnencryptedOutput,
  });
}

/** Opens a PDF in this window's backend registry and returns its view. */
export async function openDocument(
  path: string,
  password: string | null = null,
): Promise<DocumentView> {
  return invoke<DocumentView>("open_document", { path, password });
}

/** Opens another application window that loads the given PDF. */
export async function openDocumentInNewWindow(path: string): Promise<void> {
  return invoke<void>("open_document_window", { path });
}

/** Renders one page at the given scale (72 DPI = 1). */
export async function renderPage(
  documentId: string,
  pageIndex: number,
  scale: number,
): Promise<PageImageView> {
  return invoke<PageImageView>("render_page", { documentId, pageIndex, scale });
}

/** Reads display dimensions, rotation and CropBox origin for one page. */
export async function pageInfo(
  documentId: string,
  pageIndex: number,
): Promise<PageInfo> {
  return invoke<PageInfo>("page_info", { documentId, pageIndex });
}

/**
 * Renders a low-resolution page whose long edge is at most `maxEdge` pixels.
 * Thumbnails intentionally bypass the normal scale floor.
 */
export async function renderThumbnail(
  documentId: string,
  pageIndex: number,
  maxEdge: number,
): Promise<PageImageView> {
  return invoke<PageImageView>("render_thumbnail", {
    documentId,
    pageIndex,
    maxEdge,
  });
}

/** Rotates one page clockwise by 90, 180 or 270 degrees. */
export async function rotatePage(
  documentId: string,
  pageIndex: number,
  deltaDegrees: 90 | 180 | 270,
): Promise<PageOperationView> {
  return invoke<PageOperationView>("rotate_page", {
    documentId,
    pageIndex,
    deltaDegrees,
  });
}

/** Hides one page. Undo restores it with its content and annotations. */
export async function deletePage(
  documentId: string,
  pageIndex: number,
): Promise<PageOperationView> {
  return invoke<PageOperationView>("delete_page", { documentId, pageIndex });
}

/** Moves the page at `fromIndex` to the slot `toIndex`. */
export async function movePage(
  documentId: string,
  fromIndex: number,
  toIndex: number,
): Promise<PageOperationView> {
  return invoke<PageOperationView>("move_page", {
    documentId,
    fromIndex,
    toIndex,
  });
}

/** Inserts a blank page at `pageIndex` (0..=pageCount) with a paper size. */
export async function insertBlankPage(
  documentId: string,
  pageIndex: number,
  size: BlankPageSize,
): Promise<PageOperationView> {
  return invoke<PageOperationView>("insert_blank_page", {
    documentId,
    pageIndex,
    size,
  });
}

/**
 * Duplicates the page at `pageIndex` immediately after it.
 *
 * The duplicate is an independent page: its content, annotations, links and
 * form fields are copied, so editing one page leaves the other unchanged.
 * The returned selection is the inserted copy.
 */
export async function copyPage(
  documentId: string,
  pageIndex: number,
): Promise<PageOperationView> {
  return invoke<PageOperationView>("copy_page", { documentId, pageIndex });
}

/**
 * Renders only a viewport-sized region of a page, in display pixels at
 * `scale`. Used at high zoom where a full-page bitmap would be too large.
 */
export async function renderPageRegion(
  documentId: string,
  pageIndex: number,
  scale: number,
  x: number,
  y: number,
  width: number,
  height: number,
): Promise<PageRegionView> {
  return invoke<PageRegionView>("render_page_region", {
    documentId,
    pageIndex,
    scale,
    x,
    y,
    width,
    height,
  });
}

/** Reads a page's text layer; scanned pages legitimately return empty text. */
export async function extractText(
  documentId: string,
  pageIndex: number,
): Promise<ExtractedTextView> {
  return invoke<ExtractedTextView>("extract_text", { documentId, pageIndex });
}

/**
 * Reads the positioned text layer used for hit-testing, selection and copy.
 * Scanned pages return `hasText: false` with no characters.
 */
export async function extractTextLayer(
  documentId: string,
  pageIndex: number,
): Promise<TextLayerView> {
  return invoke<TextLayerView>("extract_text_layer", { documentId, pageIndex });
}

/**
 * Resolves the Text Object under a text-layer character into the initial Text
 * Box state. `charIndex` is the index in `TextLayerView.chars`.
 */
export async function textEditTarget(
  documentId: string,
  pageIndex: number,
  charIndex: number,
): Promise<TextBoxTargetView> {
  return invoke<TextBoxTargetView>("text_edit_target", {
    documentId,
    pageIndex,
    charIndex,
  });
}

/**
 * Replaces a Text Box with `text`, wrapping at its width. Resolves with the
 * document-wide history state after the edit.
 */
export async function applyTextEdit(
  documentId: string,
  pageIndex: number,
  boxId: number,
  text: string,
): Promise<TextEditResultView> {
  return invoke<TextEditResultView>("apply_text_edit", {
    documentId,
    pageIndex,
    boxId,
    text,
  });
}

/** Removes a Text Box and its original page objects from the page content. */
export async function deleteTextBox(
  documentId: string,
  pageIndex: number,
  boxId: number,
): Promise<TextEditResultView> {
  return invoke<TextEditResultView>("delete_text_box", {
    documentId,
    pageIndex,
    boxId,
  });
}

/**
 * Lists the fonts the Text Box editor may use: the PDF base-14 Latin families
 * and the bundled Noto Sans SC CJK family. The metadata carries `source`,
 * `embedded` and `coverage` so the panel can describe each family honestly.
 */
export async function textBoxFonts(): Promise<TextBoxFontView[]> {
  return invoke<TextBoxFontView[]>("text_box_fonts");
}

/** Lists the Text Boxes this session created on a page, including empty ones. */
export async function textBoxes(
  documentId: string,
  pageIndex: number,
): Promise<TextBoxTargetView[]> {
  return invoke<TextBoxTargetView[]>("text_boxes", { documentId, pageIndex });
}

/**
 * Adds an empty Text Box and records its creation in document-wide history.
 * The backend gives it the default visible style unless `style` is supplied.
 */
export async function createTextBox(
  documentId: string,
  pageIndex: number,
  request: CreateTextBoxRequest,
): Promise<TextBoxEditResultView> {
  return invoke<TextBoxEditResultView>("create_text_box", {
    documentId,
    pageIndex,
    request,
  });
}

/** Replaces the whole-box style; text reflows immediately inside the box. */
export async function applyTextBoxStyle(
  documentId: string,
  pageIndex: number,
  boxId: number,
  style: TextBoxStyleView,
): Promise<TextBoxEditResultView> {
  return invoke<TextBoxEditResultView>("apply_text_box_style", {
    documentId,
    pageIndex,
    boxId,
    style,
  });
}

/**
 * Styles only `range` inside a Text Box; the rest keeps the box's base style.
 *
 * `range` is in Unicode code points. The backend validates it, renders the
 * selected characters with their own font, size and colour, and records the
 * change as one undoable document action.
 */
export async function applyTextBoxSelectionStyle(
  documentId: string,
  pageIndex: number,
  boxId: number,
  range: TextRangeView,
  style: TextBoxStyleView,
): Promise<TextBoxEditResultView> {
  return invoke<TextBoxEditResultView>("apply_text_box_selection_style", {
    documentId,
    pageIndex,
    boxId,
    range,
    style,
  });
}

/** Moves, resizes or rotates a Text Box as one undoable document action. */
export async function transformTextBox(
  documentId: string,
  pageIndex: number,
  boxId: number,
  transform: TextBoxTransformRequest,
): Promise<TextBoxEditResultView> {
  return invoke<TextBoxEditResultView>("transform_text_box", {
    documentId,
    pageIndex,
    boxId,
    transform,
  });
}

/**
 * Inserts a PNG or JPEG image on a Page. The backend decodes and validates the
 * file before touching the document; a rejected file leaves it unchanged. The
 * image is centered and fitted inside the page on insertion.
 */
export async function insertImage(
  documentId: string,
  pageIndex: number,
  path: string,
): Promise<ImageEditResultView> {
  return invoke<ImageEditResultView>("insert_image", {
    documentId,
    pageIndex,
    path,
  });
}

/**
 * Inserts a signature on a Page from an in-memory PNG or JPEG payload.
 *
 * The dialog hands the command base64 bytes, never a file path and never a
 * log line. The backend decodes and validates them before opening the
 * document, and records the insertion as one undoable image action.
 */
export async function insertSignatureImage(
  documentId: string,
  pageIndex: number,
  imageBase64: string,
  name: string,
): Promise<ImageEditResultView> {
  return invoke<ImageEditResultView>("insert_signature_image", {
    documentId,
    pageIndex,
    imageBase64,
    name,
  });
}

/** Lists the images this editing session inserted on a page. */
export async function pageImages(
  documentId: string,
  pageIndex: number,
): Promise<ImageObjectView[]> {
  return invoke<ImageObjectView[]>("page_images", { documentId, pageIndex });
}

/** Moves, resizes or rotates an image as one undoable document action. */
export async function transformImage(
  documentId: string,
  pageIndex: number,
  imageId: number,
  transform: ImageTransformRequest,
): Promise<ImageEditResultView> {
  return invoke<ImageEditResultView>("transform_image", {
    documentId,
    pageIndex,
    imageId,
    transform,
  });
}

/** Removes an image from the page; undo restores it exactly. */
export async function deleteImage(
  documentId: string,
  pageIndex: number,
  imageId: number,
): Promise<EditStepView> {
  return invoke<EditStepView>("delete_image", {
    documentId,
    pageIndex,
    imageId,
  });
}

/**
 * Applies a watermark to the current Page or every visible logical Page. One
 * watermark is active per editing session: applying again replaces it instead
 * of stacking. The command is one undoable history action.
 */
export async function applyWatermark(
  documentId: string,
  pageIndex: number,
  request: WatermarkRequest,
): Promise<WatermarkEditResult> {
  return invoke<WatermarkEditResult>("apply_watermark", {
    documentId,
    pageIndex,
    request,
  });
}

/** Reads the active watermark, or null when none is active. */
export async function watermarkState(
  documentId: string,
): Promise<WatermarkView | null> {
  return invoke<WatermarkView | null>("watermark_state", { documentId });
}

/** Removes the active watermark as one undoable history action. */
export async function deleteWatermark(
  documentId: string,
): Promise<WatermarkEditResult> {
  return invoke<WatermarkEditResult>("delete_watermark", { documentId });
}

/**
 * Validates an image file and returns its bytes as a data URL for the panel's
 * live preview. The file is not added to the document until Apply.
 */
export async function watermarkImagePreview(
  path: string,
): Promise<WatermarkImagePreview> {
  return invoke<WatermarkImagePreview>("watermark_image_preview", { path });
}

/**
 * Draws a line or arrow as one undoable page-content object. The object is
 * written into the page content stream, never into the annotation dictionary.
 */
export async function createLineObject(
  documentId: string,
  request: CreateLineRequest,
): Promise<LineEditResultView> {
  return invoke<LineEditResultView>("create_line_object", {
    documentId,
    request,
  });
}

/** Lists the lines and arrows this session created on a page. */
export async function pageLineObjects(
  documentId: string,
  pageIndex: number,
): Promise<LineObjectView[]> {
  return invoke<LineObjectView[]>("page_line_objects", { documentId, pageIndex });
}

/**
 * Replaces a line's endpoints. Moving, endpoint dragging and rotation all
 * commit through this one command, so each gesture is one history action.
 */
export async function transformLineObject(
  documentId: string,
  pageIndex: number,
  lineId: number,
  transform: LineTransformRequest,
): Promise<LineEditResultView> {
  return invoke<LineEditResultView>("transform_line_object", {
    documentId,
    pageIndex,
    lineId,
    transform,
  });
}

/** Changes a line's stroke colour, opacity or width as one undoable action. */
export async function updateLineStyle(
  documentId: string,
  pageIndex: number,
  lineId: number,
  style: LineStyle,
): Promise<LineEditResultView> {
  return invoke<LineEditResultView>("update_line_style", {
    documentId,
    pageIndex,
    lineId,
    style,
  });
}

/** Removes a line from the page content; undo restores it exactly. */
export async function deleteLineObject(
  documentId: string,
  pageIndex: number,
  lineId: number,
): Promise<EditStepView> {
  return invoke<EditStepView>("delete_line_object", {
    documentId,
    pageIndex,
    lineId,
  });
}

/** Reads document-wide undo/redo availability. */
export async function editHistory(documentId: string): Promise<EditHistoryView> {
  return invoke<EditHistoryView>("edit_history", { documentId });
}

/** Undoes the most recent edit action and returns the affected page. */
export async function undoTextEdit(documentId: string): Promise<EditStepView> {
  return invoke<EditStepView>("undo_text_edit", { documentId });
}

/** Redoes the most recently undone edit action. */
export async function redoTextEdit(documentId: string): Promise<EditStepView> {
  return invoke<EditStepView>("redo_text_edit", { documentId });
}

/**
 * Lists every existing AcroForm widget in document order. Widgets on other
 * pages are included so the caller can plan document-wide tab order.
 */
export async function formFields(documentId: string): Promise<FormFieldView[]> {
  return invoke<FormFieldView[]>("form_fields", { documentId });
}

/**
 * Lists the required fields that still need an answer, in tab order. The save
 * flow calls this before writing so an incomplete form can be reported and
 * the first missing widget located.
 */
export async function missingRequiredFields(
  documentId: string,
): Promise<FormFieldView[]> {
  return invoke<FormFieldView[]>("missing_required_fields", { documentId });
}

/**
 * Commits one widget value. The backend records the change in the shared
 * undo/redo history and returns every field, plus the new history state.
 */
export async function fillFormField(
  documentId: string,
  fieldId: string,
  value: FormFieldValueInput,
): Promise<FormFieldEditResultView> {
  return invoke<FormFieldEditResultView>("fill_form_field", {
    documentId,
    fieldId,
    value,
  });
}

/**
 * Places a new text Form Field widget. Only documents whose permission set
 * allows changing content accept the command; the returned list already
 * contains the new field so the overlay can show and fill it immediately.
 */
export async function createFormTextField(
  documentId: string,
  pageIndex: number,
  request: CreateFormTextFieldRequest,
): Promise<CreateFormTextFieldResult> {
  return invoke<CreateFormTextFieldResult>("create_form_text_field", {
    documentId,
    pageIndex,
    request,
  });
}

/**
 * Places a new Form Field widget of any supported kind. Only documents whose
 * permission set allows changing content accept the command; the returned
 * list already contains the new field so the overlay can show and fill it
 * immediately.
 */
export async function createFormField(
  documentId: string,
  pageIndex: number,
  request: CreateFormFieldRequest,
): Promise<CreateFormFieldResult> {
  return invoke<CreateFormFieldResult>("create_form_field", {
    documentId,
    pageIndex,
    request,
  });
}

/**
 * Replaces the properties of a field created in this session, including a
 * radio group's full option list and default. A duplicate or invalid name is
 * rejected without changing the field or the history.
 */
export async function updateFormField(
  documentId: string,
  fieldId: string,
  request: UpdateFormFieldRequest,
): Promise<FormStructureEditResult> {
  return invoke<FormStructureEditResult>("update_form_field", {
    documentId,
    fieldId,
    request,
  });
}

/** Deletes a field created in this session; the action is undoable. */
export async function deleteFormField(
  documentId: string,
  fieldId: string,
): Promise<FormStructureEditResult> {
  return invoke<FormStructureEditResult>("delete_form_field", {
    documentId,
    fieldId,
  });
}

/**
 * Replaces the properties of a field created in this session. A duplicate or
 * invalid name is rejected without changing the field or the history.
 */
export async function updateFormTextField(
  documentId: string,
  fieldId: string,
  request: UpdateFormTextFieldRequest,
): Promise<FormStructureEditResult> {
  return invoke<FormStructureEditResult>("update_form_text_field", {
    documentId,
    fieldId,
    request,
  });
}

/** Deletes a field created in this session; the action is undoable. */
export async function deleteFormTextField(
  documentId: string,
  fieldId: string,
): Promise<FormStructureEditResult> {
  return invoke<FormStructureEditResult>("delete_form_text_field", {
    documentId,
    fieldId,
  });
}

/**
 * Records that the current in-memory state was written to disk. The save
 * pipeline must call this after a successful write; it moves the clean
 * baseline without clearing undo/redo, so history survives Save Changes.
 *
 * The `save_document` command performs the same baseline update internally
 * while it holds the document, so the frontend uses the history it returns
 * instead of making a second call.
 */
export async function markDocumentSaved(
  documentId: string,
): Promise<EditHistoryView> {
  return invoke<EditHistoryView>("mark_document_saved", { documentId });
}

/**
 * Creates a native highlight, underline, strikeout or squiggly over a text
 * selection.
 * The author comes from the persisted settings on the Rust side; the frontend
 * never supplies or sees it as an input.
 */
export async function createTextMarkupAnnotation(
  documentId: string,
  request: CreateTextMarkupRequest,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("create_text_markup_annotation", {
    documentId,
    request,
  });
}

/**
 * Places one built-in stamp on a page as a native Stamp annotation. The author
 * comes from the persisted settings on the Rust side.
 */
export async function createStampAnnotation(
  documentId: string,
  request: CreateStampRequest,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("create_stamp_annotation", {
    documentId,
    request,
  });
}

/** Moves, resizes or rotates a stamp in one undoable step. */
export async function transformStampAnnotation(
  documentId: string,
  request: TransformStampRequest,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("transform_stamp_annotation", {
    documentId,
    request,
  });
}

/** Lists every annotation in the document, in page order. */
export async function listAnnotations(
  documentId: string,
): Promise<AnnotationView[]> {
  return invoke<AnnotationView[]>("list_annotations", { documentId });
}

/** Lists every native link in the document, in page order. */
export async function listLinks(documentId: string): Promise<LinkView[]> {
  return invoke<LinkView[]>("list_links", { documentId });
}

/**
 * Adds one native link over a text selection's quads or an image's bounds.
 * A URL target is written as a URI action; a page target is materialized as
 * an in-document GoTo destination by the save pipeline.
 */
export async function createLink(
  documentId: string,
  request: CreateLinkRequest,
): Promise<LinkMutationView> {
  return invoke<LinkMutationView>("create_link", { documentId, request });
}

/** Replaces one existing link's target as a single undoable action. */
export async function updateLinkTarget(
  documentId: string,
  request: UpdateLinkTargetRequest,
): Promise<LinkMutationView> {
  return invoke<LinkMutationView>("update_link_target", { documentId, request });
}

/** Removes one link from the in-memory document; undo can bring it back. */
export async function deleteLink(
  documentId: string,
  linkId: string,
): Promise<LinkMutationView> {
  return invoke<LinkMutationView>("delete_link", { documentId, linkId });
}

/**
 * Places a sticky note. The author comes from persisted settings on the Rust
 * side, exactly like text markup.
 */
export async function createNoteAnnotation(
  documentId: string,
  request: CreateNoteRequest,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("create_note_annotation", {
    documentId,
    request,
  });
}

/**
 * Draws a native rectangle (Square) or ellipse (Circle) on one page. The
 * author comes from the persisted settings on the Rust side.
 */
export async function createShapeAnnotation(
  documentId: string,
  request: CreateShapeRequest,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("create_shape_annotation", {
    documentId,
    request,
  });
}

/**
 * Draws a native ink annotation. The author comes from persisted settings on
 * the Rust side.
 */
export async function createInkAnnotation(
  documentId: string,
  request: CreateInkRequest,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("create_ink_annotation", {
    documentId,
    request,
  });
}

/**
 * Places a native free text box or callout box. The author comes from the
 * persisted settings on the Rust side, exactly like text markup.
 */
export async function createFreeTextAnnotation(
  documentId: string,
  request: CreateFreeTextRequest,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("create_free_text_annotation", {
    documentId,
    request,
  });
}

/** Replaces a free text body, alignment, font size, colour and opacity. */
export async function updateFreeText(
  documentId: string,
  request: UpdateFreeTextRequest,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("update_free_text", {
    documentId,
    request,
  });
}

/** Moves or resizes a free text or callout box as one undoable action. */
export async function transformFreeTextAnnotation(
  documentId: string,
  name: string,
  rect: TextMarkupRect,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("transform_free_text_annotation", {
    documentId,
    name,
    rect,
  });
}

/** Replaces a sticky note's body as one undoable action. */
export async function updateNoteText(
  documentId: string,
  name: string,
  text: string,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("update_note_text", {
    documentId,
    name,
    text,
  });
}

/**
 * Moves and resizes a sticky note as one undoable action. `rect` is in
 * display-space points.
 */
export async function transformNoteAnnotation(
  documentId: string,
  name: string,
  rect: TextMarkupRect,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("transform_note_annotation", {
    documentId,
    name,
    rect,
  });
}

/** Moves, resizes or rotates a shape as one undoable action. */
export async function updateShapeGeometry(
  documentId: string,
  name: string,
  geometry: ShapeGeometry,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("update_shape_geometry", {
    documentId,
    name,
    geometry,
  });
}

/** Changes a shape's fill, stroke, opacity or line width as one undoable action. */
export async function updateShapeStyle(
  documentId: string,
  name: string,
  style: ShapeStyle,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("update_shape_style", {
    documentId,
    name,
    style,
  });
}

/**
 * Changes an existing annotation's colour and opacity (and, for ink only, its
 * line width) as one undoable action.
 */
export async function updateAnnotationStyle(
  documentId: string,
  name: string,
  color: string,
  opacity: number,
  lineWidth: number | null = null,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("update_annotation_style", {
    documentId,
    name,
    color,
    opacity,
    lineWidth,
  });
}

/** Removes an annotation from the in-memory document; undo can bring it back. */
export async function deleteAnnotation(
  documentId: string,
  name: string,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("delete_annotation", {
    documentId,
    name,
  });
}

/**
 * Removes one stroke from an ink annotation and rebuilds its `/InkList`,
 * bounds and appearance. Erasing the last stroke deletes the annotation.
 * Either outcome is one undoable annotation-history action.
 */
export async function eraseInkStroke(
  documentId: string,
  name: string,
  strokeIndex: number,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("erase_ink_stroke", {
    documentId,
    name,
    strokeIndex,
  });
}

/** Lists every optional-content layer in the open document. */
export async function listLayers(
  documentId: string,
): Promise<LayerListView> {
  return invoke<LayerListView>("list_layers", { documentId });
}

/**
 * Shows or hides one layer. The canvas and thumbnails repaint from the
 * session's layer state; the change is saved with the next save.
 */
export async function setLayerVisible(
  documentId: string,
  layerIndex: number,
  visible: boolean,
): Promise<LayerEditResultView> {
  return invoke<LayerEditResultView>("set_layer_visible", {
    documentId,
    layerIndex,
    visible,
  });
}

/**
 * Renames one layer. The backend trims the name and rejects blank,
 * control-containing and over-long names as `layer_name_invalid`.
 */
export async function renameLayer(
  documentId: string,
  layerIndex: number,
  name: string,
): Promise<LayerEditResultView> {
  return invoke<LayerEditResultView>("rename_layer", {
    documentId,
    layerIndex,
    name,
  });
}

/**
 * Deletes one layer and its content. The UI owns the confirmation prompt.
 * The backend refuses structures it cannot safely rewrite as
 * `layer_delete_unsupported`, leaving the document unchanged.
 */
export async function deleteLayer(
  documentId: string,
  layerIndex: number,
): Promise<LayerEditResultView> {
  return invoke<LayerEditResultView>("delete_layer", {
    documentId,
    layerIndex,
  });
}

/**
 * One file embedded in the document as an attachment. `id` is stable for the
 * lifetime of the open document; `size` is the byte length.
 */
export interface AttachmentView {
  id: number;
  name: string;
  size: number;
  /** MIME type derived from the file name. */
  mediaType: string;
}

/** The outcome of adding or deleting an attachment. */
export interface AttachmentEditResultView {
  attachments: AttachmentView[];
  history: EditHistoryView;
}

/** The outcome of extracting one attachment to a user-chosen file. */
export interface AttachmentSaveView {
  path: string;
  bytesWritten: number;
}

/** Lists the document's attachments, existing and added this session. */
export async function listAttachments(
  documentId: string,
): Promise<AttachmentView[]> {
  return invoke<AttachmentView[]>("list_attachments", { documentId });
}

/**
 * Adds the chosen file to the session as an undoable action. The attachment
 * is embedded in the PDF only when the document is saved.
 */
export async function addAttachment(
  documentId: string,
  path: string,
): Promise<AttachmentEditResultView> {
  return invoke<AttachmentEditResultView>("add_attachment", {
    documentId,
    path,
  });
}

/**
 * Deletes one attachment from the session as an undoable action. The UI asks
 * for confirmation before calling this command.
 */
export async function deleteAttachment(
  documentId: string,
  attachmentId: number,
): Promise<AttachmentEditResultView> {
  return invoke<AttachmentEditResultView>("delete_attachment", {
    documentId,
    attachmentId,
  });
}

/**
 * Extracts one attachment to a temporary location and opens it with the OS
 * default handler. Attachments are never opened automatically: `confirm` must
 * be true after the user accepted the warning.
 */
export async function openAttachment(
  documentId: string,
  attachmentId: number,
  confirm: boolean,
): Promise<void> {
  return invoke<void>("open_attachment", {
    documentId,
    attachmentId,
    confirm,
  });
}

/**
 * Extracts one attachment to `destination` with an atomic write. The Source
 * Document and the current Output Document are refused.
 */
export async function saveAttachmentAs(
  documentId: string,
  attachmentId: number,
  destination: string,
): Promise<AttachmentSaveView> {
  return invoke<AttachmentSaveView>("save_attachment_as", {
    documentId,
    attachmentId,
    destination,
  });
}

/**
 * Opens the Save As dialog filtered to PDFs. Returns null when cancelled.
 * `defaultName` preselects the Source Document's file name.
 */
export async function chooseSaveFile(
  defaultName: string | null = null,
): Promise<string | null> {
  const selection = await saveDialog({
    defaultPath: defaultName ?? undefined,
    filters: [{ name: "PDF", extensions: ["pdf"] }],
  });
  return typeof selection === "string" ? selection : null;
}

/**
 * Writes the Output Document. Save As passes the chosen path; Save Changes
 * passes null and overwrites the remembered output. An encrypted Source
 * Document is written unencrypted only when `confirmUnencryptedOutput` is true.
 */
export async function saveDocument(
  documentId: string,
  request: SaveDocumentRequest,
): Promise<SaveResultView> {
  return invoke<SaveResultView>("save_document", { documentId, request });
}

/** Subscribes to save progress for the current window. */
export function onSaveProgress(
  handler: (progress: SaveProgress) => void,
): Promise<UnlistenFn> {
  return listen<SaveProgress>(SAVE_PROGRESS_EVENT, (event) => handler(event.payload));
}

/** Releases a document from the backend registry. Unknown ids are ignored. */
export async function closeDocument(documentId: string): Promise<void> {
  return invoke<void>("close_document", { documentId });
}

/**
 * Runs one throttled Recovery File tick.
 *
 * The backend decides whether anything is written (debounce plus a maximum
 * interval); repeated calls while editing are cheap and never write per
 * keystroke. The command is async so serialization does not block the UI.
 */
export async function recoverySnapshot(
  documentId: string,
): Promise<RecoverySnapshotOutcome> {
  return invoke<RecoverySnapshotOutcome>("recovery_snapshot", { documentId });
}

/**
 * Lists Recovery Files left by an abnormal exit, newest first. Reading the
 * list never opens a document; the start-up prompt decides what to do.
 */
export async function listRecoveryCandidates(): Promise<RecoverySnapshotView[]> {
  return invoke<RecoverySnapshotView[]>("recovery_candidates");
}

/**
 * Opens a Recovery File as the Document. Unsaved edits are restored, the
 * original Source path is provenance only, and saving uses the normal pipeline.
 */
export async function restoreRecovery(sessionId: string): Promise<DocumentView> {
  return invoke<DocumentView>("restore_recovery", { sessionId });
}

/** Removes one Recovery File without opening it. */
export async function discardRecovery(sessionId: string): Promise<void> {
  return invoke<void>("discard_recovery", { sessionId });
}

/** Reads the locally persisted recent-files list (missing files pruned). */
export async function listRecentFiles(): Promise<RecentFileView[]> {
  return invoke<RecentFileView[]>("list_recent_files");
}

/** Removes one path from the recent-files list and returns the new list. */
export async function forgetRecentFile(
  path: string,
): Promise<RecentFileView[]> {
  return invoke<RecentFileView[]>("forget_recent_file", { path });
}

/** Sets the native window title (the native menu shows the same name). */
export async function setWindowTitle(title: string): Promise<void> {
  return getCurrentWindow().setTitle(title);
}

/**
 * Destroys the current window without emitting another close-requested round
 * trip. The close guard calls this once it has released the Document, so the
 * window cannot be closed a second time by the default event handling.
 */
export async function destroyWindow(): Promise<void> {
  return getCurrentWindow().destroy();
}

/** Enters or leaves native fullscreen for the current window. */
export async function setFullscreen(fullscreen: boolean): Promise<void> {
  return getCurrentWindow().setFullscreen(fullscreen);
}

/** Whether the current window is in native fullscreen. */
export async function isFullscreen(): Promise<boolean> {
  return getCurrentWindow().isFullscreen();
}

/** Subscribes to native menu activations, including keyboard accelerators. */
export function onMenuEvent(handler: (menuId: string) => void): Promise<UnlistenFn> {
  return listen<string>(MENU_EVENT, (event) => handler(event.payload));
}

/** One native menu item's enabled flag in a menu-state snapshot. */
export interface MenuItemState {
  id: string;
  enabled: boolean;
}

/**
 * Applies the workspace's menu availability to the native menu.
 *
 * The native menu lives in Rust; this is the only command that changes which
 * items can fire. The workspace disables an item whenever its command would be
 * invalid, and the menu layer releases that item's accelerator with it.
 */
export async function setMenuState(items: MenuItemState[]): Promise<void> {
  return invoke<void>("set_menu_state", { state: { items } });
}

/** Subscribes to file paths dropped onto the window. */
export function onFileDrop(handler: (paths: string[]) => void): Promise<UnlistenFn> {
  return getCurrentWebview().onDragDropEvent((event) => {
    if (event.payload.type === "drop") {
      handler(event.payload.paths);
    }
  });
}

/** Runs the handler when the OS asks this window to close. */
export function onWindowCloseRequested(
  handler: (event: { preventDefault: () => void }) => void | Promise<void>,
): Promise<UnlistenFn> {
  return getCurrentWindow().onCloseRequested(handler);
}

/** The document passed to a freshly created window via `?open=<path>`. */
export function initialDocumentPath(): string | null {
  const path = new URLSearchParams(window.location.search).get("open");
  return path && path.length > 0 ? path : null;
}

/**
 * Normalizes anything thrown by `invoke` into the structured command error.
 * Tauri serializes Rust `CommandError`s as plain objects, not `Error`s.
 */
export function toCommandError(error: unknown): CommandError {
  if (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    "message" in error &&
    typeof (error as { code: unknown }).code === "string" &&
    typeof (error as { message: unknown }).message === "string" &&
    COMMAND_ERROR_CODES.has((error as { code: string }).code)
  ) {
    return {
      code: (error as { code: CommandErrorCode }).code,
      message: (error as { message: string }).message,
    };
  }

  if (error instanceof Error) {
    return { code: "internal", message: error.message };
  }

  return { code: "internal", message: String(error) };
}
