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
  | "render_failed"
  | "window_failed"
  | "internal";

/** A structured error crossing the Tauri command boundary. */
export interface CommandError {
  code: CommandErrorCode;
  message: string;
}

export interface DocumentView {
  documentId: string;
  name: string;
  path: string;
  pageCount: number;
  pageWidth: number;
  pageHeight: number;
}

export interface PageImageView {
  documentId: string;
  pageIndex: number;
  scale: number;
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

/** Event emitted to the focused window for native menu activation. */
export const MENU_EVENT = "pdfx://menu";

/** Zoom is relative to 72 DPI; 1 is actual size. */
export const DEFAULT_SCALE = 1;
export const MIN_SCALE = 0.25;
export const MAX_SCALE = 4;
export const ZOOM_STEP = 1.25;

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

/** Releases a document from the backend registry. Unknown ids are ignored. */
export async function closeDocument(documentId: string): Promise<void> {
  return invoke<void>("close_document", { documentId });
}

/** Sets the native window title (the native menu shows the same name). */
export async function setWindowTitle(title: string): Promise<void> {
  return getCurrentWindow().setTitle(title);
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
