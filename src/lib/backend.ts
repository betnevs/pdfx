import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

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
  | "render_failed"
  | "window_failed"
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
 * `coverage` states which characters the font can render. Ticket 11 adds
 * embedded families without changing this shape.
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
  "render_failed",
  "window_failed",
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
 * Lists the fonts the Text Box editor may use. Until the subset-embedding
 * pipeline lands these are the PDF base-14 Latin families only; the metadata
 * already carries `source`, `embedded` and `coverage` for later families.
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
 */
export async function markDocumentSaved(
  documentId: string,
): Promise<EditHistoryView> {
  return invoke<EditHistoryView>("mark_document_saved", { documentId });
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
