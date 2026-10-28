import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  choosePdfFile,
  closeDocument,
  initialDocumentPath,
  MENU_EVENT,
  onFileDrop,
  onMenuEvent,
  openDocument,
  openDocumentInNewWindow,
  renderPage,
  setWindowTitle,
  toCommandError,
} from "./backend";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const invokeMock = vi.mocked(invoke);

describe("backend command seam", () => {
  beforeEach(() => {
    invokeMock.mockResolvedValue(undefined);
  });

  afterEach(() => {
    invokeMock.mockReset();
    vi.mocked(openDialog).mockReset();
    vi.mocked(listen).mockReset();
    vi.mocked(getCurrentWindow).mockReset();
    vi.mocked(getCurrentWebview).mockReset();
    window.history.replaceState({}, "", "/");
  });

  it("asks the file dialog for exactly one PDF", async () => {
    vi.mocked(openDialog).mockResolvedValue("/tmp/report.pdf");

    await expect(choosePdfFile()).resolves.toBe("/tmp/report.pdf");
    expect(openDialog).toHaveBeenCalledWith({
      multiple: false,
      directory: false,
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
  });

  it("returns null when the file dialog is cancelled", async () => {
    vi.mocked(openDialog).mockResolvedValue(null);

    await expect(choosePdfFile()).resolves.toBeNull();
  });

  it("maps document commands to their Tauri command names and camelCase arguments", async () => {
    invokeMock.mockResolvedValue({
      documentId: "doc-1",
      name: "report.pdf",
      path: "/tmp/report.pdf",
      pageCount: 3,
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
    });

    const view = await openDocument("/tmp/report.pdf");
    expect(view.documentId).toBe("doc-1");
    expect(invokeMock).toHaveBeenCalledWith("open_document", {
      path: "/tmp/report.pdf",
      password: null,
    });

    invokeMock.mockResolvedValue({
      documentId: "doc-1",
      pageIndex: 0,
      scale: 1,
      width: 612,
      height: 792,
      pngBase64: "iVBORw0KGgo=",
    });
    const page = await renderPage("doc-1", 0, 1);
    expect(page.pngBase64).toBe("iVBORw0KGgo=");
    expect(invokeMock).toHaveBeenCalledWith("render_page", {
      documentId: "doc-1",
      pageIndex: 0,
      scale: 1,
    });

    await closeDocument("doc-1");
    expect(invokeMock).toHaveBeenCalledWith("close_document", { documentId: "doc-1" });
  });

  it("forwards a supplied password to open_document", async () => {
    invokeMock.mockResolvedValue({
      documentId: "doc-1",
      name: "secret.pdf",
      path: "/tmp/secret.pdf",
      pageCount: 1,
      pageWidth: 612,
      pageHeight: 792,
      encrypted: true,
      readOnly: false,
      permissions: {
        canModifyContent: true,
        canAnnotate: true,
        canAssemble: true,
        canFillForms: true,
        canPrint: true,
        canCopyContent: true,
      },
    });

    await openDocument("/tmp/secret.pdf", "pdfx-user");

    expect(invokeMock).toHaveBeenCalledWith("open_document", {
      path: "/tmp/secret.pdf",
      password: "pdfx-user",
    });
  });

  it("opens a second document window through its own command", async () => {
    await openDocumentInNewWindow("/tmp/other.pdf");

    expect(invokeMock).toHaveBeenCalledWith("open_document_window", {
      path: "/tmp/other.pdf",
    });
  });

  it("sets the current window title", async () => {
    const setTitle = vi.fn().mockResolvedValue(undefined);
    vi.mocked(getCurrentWindow).mockReturnValue({
      setTitle,
    } as unknown as ReturnType<typeof getCurrentWindow>);

    await setWindowTitle("report.pdf — PDFX");

    expect(setTitle).toHaveBeenCalledWith("report.pdf — PDFX");
  });

  it("forwards menu payloads and unsubscribes", async () => {
    const unlisten = vi.fn();
    let handler: ((event: unknown) => void) | undefined;
    vi.mocked(listen).mockImplementation(async (event, callback) => {
      expect(event).toBe(MENU_EVENT);
      handler = callback as (event: unknown) => void;
      return unlisten;
    });
    const received: string[] = [];

    const dispose = await onMenuEvent((id) => received.push(id));
    handler?.({ payload: "view.zoom_in" });

    expect(received).toEqual(["view.zoom_in"]);
    dispose();
    expect(unlisten).toHaveBeenCalled();
  });

  it("forwards only paths from a drag-drop event", async () => {
    let handler: ((event: unknown) => void) | undefined;
    vi.mocked(getCurrentWebview).mockReturnValue({
      onDragDropEvent: vi.fn(async (callback: (event: unknown) => void) => {
        handler = callback;
        return () => {};
      }),
    } as unknown as ReturnType<typeof getCurrentWebview>);
    const received: string[][] = [];

    await onFileDrop((paths) => received.push(paths));

    handler?.({ payload: { type: "enter", paths: ["/tmp/hover.pdf"], position: { x: 1, y: 2 } } });
    handler?.({ payload: { type: "drop", paths: ["/tmp/dropped.pdf"], position: { x: 1, y: 2 } } });

    expect(received).toEqual([["/tmp/dropped.pdf"]]);
  });

  it("reads the document path from the window query string", () => {
    window.history.replaceState({}, "", "/?open=%2Ftmp%2Freport.pdf");

    expect(initialDocumentPath()).toBe("/tmp/report.pdf");
  });

  it("returns null when the window has no document query", () => {
    expect(initialDocumentPath()).toBeNull();
  });

  it("normalizes command errors and unknown failures", () => {
    expect(toCommandError({ code: "corrupt_document", message: "bad xref" })).toEqual({
      code: "corrupt_document",
      message: "bad xref",
    });
    expect(toCommandError(new Error("ipc unavailable"))).toEqual({
      code: "internal",
      message: "ipc unavailable",
    });
  });
});
