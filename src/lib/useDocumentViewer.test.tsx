import { renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DocumentView } from "./backend";
import { notifyDocumentChanged } from "./documentEvents";
import { useDocumentViewer } from "./useDocumentViewer";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const getCurrentWindowMock = vi.mocked(getCurrentWindow);

const document: DocumentView = {
  documentId: "doc-1",
  name: "sample.pdf",
  path: "/tmp/sample.pdf",
  pageCount: 1,
  pageWidth: 612,
  pageHeight: 792,
  encrypted: false,
  readOnly: false,
  permissions: {
    canModifyContent: true,
    canAnnotate: true,
    canAssemble: true,
    canFillForms: true,
    canPrint: true,
    canCopyContent: true,
  },
};

function renderPageCalls() {
  return invokeMock.mock.calls.filter(([command]) => command === "render_page");
}

beforeEach(() => {
  getCurrentWindowMock.mockReturnValue({
    isFullscreen: vi.fn().mockResolvedValue(false),
    setFullscreen: vi.fn().mockResolvedValue(undefined),
    setTitle: vi.fn().mockResolvedValue(undefined),
  } as unknown as ReturnType<typeof getCurrentWindow>);

  invokeMock.mockImplementation((command) => {
    switch (command) {
      case "page_info":
        return Promise.resolve({
          pageIndex: 0,
          width: 612,
          height: 792,
          rotation: 0,
          cropX: 0,
          cropY: 0,
        });
      case "render_page":
        return Promise.resolve({
          documentId: "doc-1",
          pageIndex: 0,
          scale: 1,
          width: 612,
          height: 792,
          pngBase64: "",
        });
      default:
        return Promise.reject(new Error(`unexpected command ${command as string}`));
    }
  });
});

afterEach(() => {
  invokeMock.mockReset();
  getCurrentWindowMock.mockReset();
});

describe("useDocumentViewer", () => {
  it("re-renders the page after its content changes", async () => {
    renderHook(() => useDocumentViewer(document, vi.fn()));

    await waitFor(() => expect(renderPageCalls()).toHaveLength(1));

    notifyDocumentChanged({ documentId: "doc-1", pageIndex: 0 });

    await waitFor(() => expect(renderPageCalls()).toHaveLength(2));
  });

  it("ignores content changes in another document", async () => {
    renderHook(() => useDocumentViewer(document, vi.fn()));
    await waitFor(() => expect(renderPageCalls()).toHaveLength(1));

    notifyDocumentChanged({ documentId: "doc-2", pageIndex: 0 });
    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(renderPageCalls()).toHaveLength(1);
  });
});
