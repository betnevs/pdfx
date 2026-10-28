import { describe, expect, it, vi } from "vitest";

import {
  DOCUMENT_CHANGED_EVENT,
  notifyDocumentChanged,
  onDocumentChanged,
} from "./documentEvents";

describe("document change events", () => {
  it("passes the source through so overlays can refresh tool state", () => {
    const handler = vi.fn();
    const unsubscribe = onDocumentChanged(handler);

    notifyDocumentChanged({ documentId: "doc-1", pageIndex: 2, source: "image" });

    expect(handler).toHaveBeenCalledWith({
      documentId: "doc-1",
      pageIndex: 2,
      source: "image",
    });
    unsubscribe();
  });

  it("passes the line source used by the content-object overlay", () => {
    const handler = vi.fn();
    const unsubscribe = onDocumentChanged(handler);

    notifyDocumentChanged({ documentId: "doc-1", pageIndex: 0, source: "line" });

    expect(handler).toHaveBeenCalledWith({
      documentId: "doc-1",
      pageIndex: 0,
      source: "line",
    });
    unsubscribe();
  });

  it("passes the link source used by the link overlay", () => {
    const handler = vi.fn();
    const unsubscribe = onDocumentChanged(handler);

    notifyDocumentChanged({ documentId: "doc-1", pageIndex: 0, source: "link" });

    expect(handler).toHaveBeenCalledWith({
      documentId: "doc-1",
      pageIndex: 0,
      source: "link",
    });
    unsubscribe();
  });

  it("omits a missing or unknown source", () => {
    const handler = vi.fn();
    const unsubscribe = onDocumentChanged(handler);

    notifyDocumentChanged({ documentId: "doc-1", pageIndex: 0 });
    window.dispatchEvent(
      new CustomEvent(DOCUMENT_CHANGED_EVENT, {
        detail: { documentId: "doc-1", pageIndex: 1, source: "bogus" },
      }),
    );

    expect(handler).toHaveBeenNthCalledWith(1, {
      documentId: "doc-1",
      pageIndex: 0,
    });
    expect(handler).toHaveBeenNthCalledWith(2, {
      documentId: "doc-1",
      pageIndex: 1,
    });
    unsubscribe();
  });

  it("ignores malformed events and stops after unsubscribe", () => {
    const handler = vi.fn();
    const unsubscribe = onDocumentChanged(handler);

    window.dispatchEvent(
      new CustomEvent(DOCUMENT_CHANGED_EVENT, { detail: { pageIndex: 0 } }),
    );
    expect(handler).not.toHaveBeenCalled();

    unsubscribe();
    notifyDocumentChanged({ documentId: "doc-1", pageIndex: 0 });
    expect(handler).not.toHaveBeenCalled();
  });
});
