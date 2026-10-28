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
  pageIndex: number;
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
      handler({ documentId: detail.documentId, pageIndex: detail.pageIndex });
    }
  };
  window.addEventListener(DOCUMENT_CHANGED_EVENT, listener);
  return () => window.removeEventListener(DOCUMENT_CHANGED_EVENT, listener);
}
