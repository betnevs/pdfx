import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import { notifyDocumentChanged } from "../lib/documentEvents";
import { ThumbnailRail } from "./PageViewer";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const invokeMock = vi.mocked(invoke);

function thumbnailCalls(pageIndex: number) {
  return invokeMock.mock.calls.filter(
    ([command, args]) =>
      command === "render_thumbnail" &&
      (args as { pageIndex: number }).pageIndex === pageIndex,
  );
}

afterEach(() => {
  invokeMock.mockReset();
});

describe("ThumbnailRail", () => {
  it("re-renders a thumbnail after that page's content changes", async () => {
    invokeMock.mockImplementation((command, args) => {
      if (command !== "render_thumbnail") {
        return Promise.reject(new Error(`unexpected command ${command as string}`));
      }
      const { pageIndex } = args as { pageIndex: number };
      return Promise.resolve({
        documentId: "doc-1",
        pageIndex,
        scale: 1,
        width: 10,
        height: 10,
        pngBase64: "",
      });
    });

    render(
      <ThumbnailRail
        documentId="doc-1"
        pageCount={1}
        currentPageIndex={0}
        onSelectPage={vi.fn()}
      />,
    );

    await waitFor(() => expect(thumbnailCalls(0)).toHaveLength(1));

    notifyDocumentChanged({ documentId: "doc-1", pageIndex: 0 });

    await waitFor(() => expect(thumbnailCalls(0)).toHaveLength(2));
  });

  it("ignores content changes in another document", async () => {
    invokeMock.mockImplementation((_command, args) => {
      const { pageIndex } = args as { pageIndex: number };
      return Promise.resolve({
        documentId: "doc-1",
        pageIndex,
        scale: 1,
        width: 10,
        height: 10,
        pngBase64: "",
      });
    });

    render(
      <ThumbnailRail
        documentId="doc-1"
        pageCount={1}
        currentPageIndex={0}
        onSelectPage={vi.fn()}
      />,
    );
    await waitFor(() => expect(thumbnailCalls(0)).toHaveLength(1));

    notifyDocumentChanged({ documentId: "doc-2", pageIndex: 0 });
    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(thumbnailCalls(0)).toHaveLength(1);
  });

  it("re-renders every thumbnail when the page arrangement changes", async () => {
    invokeMock.mockImplementation((_command, args) => {
      const { pageIndex } = args as { pageIndex: number };
      return Promise.resolve({
        documentId: "doc-1",
        pageIndex,
        scale: 1,
        width: 10,
        height: 10,
        pngBase64: "",
      });
    });

    render(
      <ThumbnailRail
        documentId="doc-1"
        pageCount={2}
        currentPageIndex={0}
        onSelectPage={vi.fn()}
      />,
    );
    await waitFor(() => expect(thumbnailCalls(0)).toHaveLength(1));
    await waitFor(() => expect(thumbnailCalls(1)).toHaveLength(1));

    notifyDocumentChanged({
      documentId: "doc-1",
      pageIndex: 0,
      pageCount: 2,
      pageStructureChanged: true,
    });

    await waitFor(() => expect(thumbnailCalls(0)).toHaveLength(2));
    await waitFor(() => expect(thumbnailCalls(1)).toHaveLength(2));
  });

  it("moves a page when a thumbnail is dropped on another slot", () => {
    invokeMock.mockImplementation((_command, args) => {
      const { pageIndex } = args as { pageIndex: number };
      return Promise.resolve({
        documentId: "doc-1",
        pageIndex,
        scale: 1,
        width: 10,
        height: 10,
        pngBase64: "",
      });
    });
    const onMovePage = vi.fn();

    render(
      <ThumbnailRail
        documentId="doc-1"
        pageCount={3}
        currentPageIndex={0}
        onSelectPage={vi.fn()}
        canReorder
        onMovePage={onMovePage}
      />,
    );

    const first = screen.getByRole("button", { name: "第 1 页" });
    const third = screen.getByRole("button", { name: "第 3 页" });
    fireEvent.dragStart(first);
    fireEvent.dragOver(third);
    fireEvent.drop(third);

    expect(onMovePage).toHaveBeenCalledWith(0, 2);
  });

  it("does not reorder pages when the document is read-only", () => {
    invokeMock.mockImplementation((_command, args) => {
      const { pageIndex } = args as { pageIndex: number };
      return Promise.resolve({
        documentId: "doc-1",
        pageIndex,
        scale: 1,
        width: 10,
        height: 10,
        pngBase64: "",
      });
    });
    const onMovePage = vi.fn();

    render(
      <ThumbnailRail
        documentId="doc-1"
        pageCount={3}
        currentPageIndex={0}
        onSelectPage={vi.fn()}
        onMovePage={onMovePage}
      />,
    );

    const first = screen.getByRole("button", { name: "第 1 页" });
    const third = screen.getByRole("button", { name: "第 3 页" });
    expect(first).toHaveAttribute("draggable", "false");
    fireEvent.dragStart(first);
    fireEvent.dragOver(third);
    fireEvent.drop(third);

    expect(onMovePage).not.toHaveBeenCalled();
  });
});
