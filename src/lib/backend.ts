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
  | "print_failed"
  | "window_failed"
  | "save_failed"
  | "save_target_is_source"
  | "no_output_document"
  | "unencrypted_output_confirmation_required"
  | "read_only_document"
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
  /** File name, shown on the overlay label. */
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  /** Clockwise rotation in degrees in display space. */
  rotation: number;
  pixelWidth: number;
  pixelHeight: number;
}

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

/** The text markup annotation kinds PDFX creates. */
export type TextMarkupKind = "highlight" | "underline" | "strikeout";

/** A display-space rectangle of a markup selection. */
export interface TextMarkupRect {
  x: number;
  y: number;
  width: number;
  height: number;
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

/**
 * A native text markup annotation read back from the in-memory document.
 *
 * `name` is the stable `/NM` identity used by locate, restyle and delete.
 * `author` and `createdAt` are copied into the PDF at creation time; `createdAt`
 * is the PDF date string (`D:YYYYMMDDHHmmSSZ`).
 */
export interface AnnotationView {
  name: string;
  pageIndex: number;
  kind: TextMarkupKind;
  author: string | null;
  createdAt: string | null;
  color: string;
  opacity: number;
  rects: TextMarkupRect[];
}

/** The outcome of creating, restyling or deleting one annotation. */
export interface AnnotationMutationView {
  /** Null after a deletion. */
  annotation: AnnotationView | null;
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
}

/** The outcome of a completed save. */
export interface SaveResultView {
  documentId: string;
  outputPath: string;
  kind: SaveKind;
  bytesWritten: number;
  /** The previous Output Document, preserved before it was replaced. */
  backupPath: string | null;
  /** Clean state after the write; undo/redo history is untouched. */
  history: EditHistoryView;
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
  "print_failed",
  "window_failed",
  "save_failed",
  "save_target_is_source",
  "no_output_document",
  "unencrypted_output_confirmation_required",
  "read_only_document",
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
 * Creates a native highlight, underline or strikeout over a text selection.
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

/** Lists every text markup annotation in the document, in page order. */
export async function listAnnotations(
  documentId: string,
): Promise<AnnotationView[]> {
  return invoke<AnnotationView[]>("list_annotations", { documentId });
}

/** Changes an existing annotation's colour and opacity as one undoable action. */
export async function updateAnnotationStyle(
  documentId: string,
  name: string,
  color: string,
  opacity: number,
): Promise<AnnotationMutationView> {
  return invoke<AnnotationMutationView>("update_annotation_style", {
    documentId,
    name,
    color,
    opacity,
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

/** Sets the native window title (the native menu shows the same name). */
export async function setWindowTitle(title: string): Promise<void> {
  return getCurrentWindow().setTitle(title);
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
  handler: (event: { preventDefault: () => void }) => void,
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
