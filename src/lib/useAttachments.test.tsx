import { act, renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { AttachmentView } from "./backend";
import { notifyDocumentChanged } from "./documentEvents";
import { useAttachments } from "./useAttachments";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

const text: AttachmentView = {
  id: 1,
  name: "report.txt",
  size: 17,
  mediaType: "text/plain",
};

const spreadsheet: AttachmentView = {
  id: 2,
  name: "data.csv",
  size: 2048,
  mediaType: "text/csv",
};

function listCalls() {
  return invokeMock.mock.calls.filter(([command]) => command === "list_attachments");
}

describe("useAttachments", () => {
  beforeEach(() => {
    invokeMock.mockResolvedValue([]);
  });

  afterEach(() => {
    invokeMock.mockReset();
  });

  it("loads the attachment list for the open document", async () => {
    invokeMock.mockResolvedValue([text]);

    const { result } = renderHook(() => useAttachments("doc-1"));

    expect(result.current.status).toBe("loading");
    expect(result.current.attachments).toEqual([]);
    await waitFor(() => expect(result.current.status).toBe("ready"));
    expect(result.current.attachments).toEqual([text]);
    expect(result.current.error).toBeNull();
    expect(invokeMock).toHaveBeenCalledWith("list_attachments", {
      documentId: "doc-1",
    });
  });

  it("stays idle without a document and reports load errors", async () => {
    const idle = renderHook(() => useAttachments(null));
    expect(idle.result.current.status).toBe("idle");
    expect(listCalls()).toHaveLength(0);
    idle.unmount();

    invokeMock.mockRejectedValue({
      code: "document_not_found",
      message: "document is closed",
    });

    const failed = renderHook(() => useAttachments("doc-1"));
    await waitFor(() => expect(failed.result.current.status).toBe("error"));
    expect(failed.result.current.error?.code).toBe("document_not_found");
    expect(failed.result.current.attachments).toEqual([]);
  });

  it("refetches when the same document reports a change", async () => {
    invokeMock.mockResolvedValue([text]);

    const { result } = renderHook(() => useAttachments("doc-1"));
    await waitFor(() => expect(result.current.status).toBe("ready"));

    invokeMock.mockResolvedValue([text, spreadsheet]);
    act(() => {
      notifyDocumentChanged({ documentId: "doc-1", pageIndex: 0 });
    });

    await waitFor(() =>
      expect(result.current.attachments).toEqual([text, spreadsheet]),
    );
    expect(listCalls()).toHaveLength(2);

    // A change in another document must not refetch this one.
    act(() => {
      notifyDocumentChanged({ documentId: "doc-2", pageIndex: 0 });
    });
    expect(listCalls()).toHaveLength(2);
  });
});
