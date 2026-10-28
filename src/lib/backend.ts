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
