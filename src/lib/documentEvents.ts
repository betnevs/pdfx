/**
 * In-window notification that a document's page content changed.
 *
 * Text edits, deletions, undo and redo all mutate the in-memory document on
 * the Rust side. The rendered page bitmap, the text layer and the thumbnail
 * for the affected page would otherwise keep showing the pre-edit content, so
 * the edit UI announces the change and each view refreshes only that page.
 *
 * This is a same-window DOM event rather than a Tauri event: it does not cross
 * windows, and a new window opens its own document. The save pipeline and
 * future overlay tools can emit the same event.
 */

export const DOCUMENT_CHANGED_EVENT = "pdfx://document-changed";

export interface DocumentChangedDetail {
  documentId: string;
  /**
   * The page to refresh. For a page operation this is the page the viewer
   * should select after the change.
   */
  pageIndex: number;
  /**
   * Visible page count after a page operation. Selection is clamped to it.
   */
  pageCount?: number;
  /**
   * Page order, count or rotation changed. Caches keyed by page index — page
   * metadata and thumbnails — must be cleared, not just the one page's pixels.
   */
  pageStructureChanged?: boolean;
  /**
   * Every page's content may have changed even though the page order did not,
   * for example after applying or removing an all-pages watermark, or after an
   * undo/redo whose affected pages are only known to the backend. Thumbnail
   * caches must be cleared for every page; page metadata is still valid.
   */
  allPagesChanged?: boolean;
  /**
   * The tool that made the change, when another overlay needs to refresh
   * tool-specific state such as the shared undo availability. Omitted by
   * callers whose command results already carry that state.
   *
   * `"form"` marks a form fill whose overlay already updated from the command
   * result; `"text"`, `"image"`, `"shape"`, `"line"`, `"link"`,
   * `"annotation"` and `"watermark"` mark the Text Box, Image, Shape,
   * Line/Arrow, link, text-markup and watermark edits whose command results
   * returned the new history.
   */
  source?:
    | "text"
    | "image"
    | "shape"
    | "line"
    | "link"
    | "annotation"
    | "watermark"
    | "form";
}

export function notifyDocumentChanged(detail: DocumentChangedDetail): void {
  window.dispatchEvent(
    new CustomEvent<DocumentChangedDetail>(DOCUMENT_CHANGED_EVENT, {
      detail,
    }),
  );
}

/**
 * Subscribes to page-content changes. Returns an unsubscribe function.
 * Malformed events (for example from third-party code) are ignored.
 */
export function onDocumentChanged(
  handler: (detail: DocumentChangedDetail) => void,
): () => void {
  const listener = (event: Event) => {
    const detail = (event as CustomEvent<Partial<DocumentChangedDetail>>).detail;
    if (
      detail &&
      typeof detail.documentId === "string" &&
      typeof detail.pageIndex === "number"
    ) {
      const message: DocumentChangedDetail = {
        documentId: detail.documentId,
        pageIndex: detail.pageIndex,
      };
      if (
        typeof detail.pageCount === "number" &&
        Number.isFinite(detail.pageCount)
      ) {
        message.pageCount = detail.pageCount;
      }
      if (detail.pageStructureChanged === true) {
        message.pageStructureChanged = true;
      }
      if (detail.allPagesChanged === true) {
        message.allPagesChanged = true;
      }
      if (
        detail.source === "text" ||
        detail.source === "image" ||
        detail.source === "shape" ||
        detail.source === "line" ||
        detail.source === "link" ||
        detail.source === "annotation" ||
        detail.source === "watermark" ||
        detail.source === "form"
      ) {
        message.source = detail.source;
      }
      handler(message);
    }
  };
  window.addEventListener(DOCUMENT_CHANGED_EVENT, listener);
  return () => window.removeEventListener(DOCUMENT_CHANGED_EVENT, listener);
}
