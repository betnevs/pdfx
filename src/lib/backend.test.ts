import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  choosePdfFile,
  chooseSaveFile,
  closeDocument,
  extractTextLayer,
  initialDocumentPath,
  isFullscreen,
  MENU_EVENT,
  onFileDrop,
  onMenuEvent,
  onSaveProgress,
  openDocument,
  openDocumentInNewWindow,
  pageInfo,
  renderPage,
  renderPageRegion,
  renderThumbnail,
  SAVE_PROGRESS_EVENT,
  saveDocument,
  setFullscreen,
  setWindowTitle,
  toCommandError,
} from "./backend";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

const invokeMock = vi.mocked(invoke);

describe("backend command seam", () => {
  beforeEach(() => {
    invokeMock.mockResolvedValue(undefined);
  });

  afterEach(() => {
    invokeMock.mockReset();
    vi.mocked(openDialog).mockReset();
    vi.mocked(saveDialog).mockReset();
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

  it("reads page metadata, thumbnails and high-zoom regions", async () => {
    invokeMock.mockResolvedValue({
      pageIndex: 2,
      width: 432,
      height: 288,
      rotation: 270,
      cropX: 0,
      cropY: 0,
    });

    const info = await pageInfo("doc-1", 2);
    expect(info).toEqual({
      pageIndex: 2,
      width: 432,
      height: 288,
      rotation: 270,
      cropX: 0,
      cropY: 0,
    });
    expect(invokeMock).toHaveBeenCalledWith("page_info", {
      documentId: "doc-1",
      pageIndex: 2,
    });

    invokeMock.mockResolvedValue({
      documentId: "doc-1",
      pageIndex: 2,
      scale: 0.277778,
      width: 120,
      height: 80,
      pngBase64: "iVBORw0KGgo=",
    });
    const thumbnail = await renderThumbnail("doc-1", 2, 120);
    expect(thumbnail.width).toBe(120);
    expect(invokeMock).toHaveBeenLastCalledWith("render_thumbnail", {
      documentId: "doc-1",
      pageIndex: 2,
      maxEdge: 120,
    });

    invokeMock.mockResolvedValue({
      documentId: "doc-1",
      pageIndex: 2,
      scale: 20,
      x: 4000,
      y: 3000,
      width: 800,
      height: 600,
      pngBase64: "iVBORw0KGgo=",
    });
    const region = await renderPageRegion("doc-1", 2, 20, 4000, 3000, 800, 600);
    expect(region.x).toBe(4000);
    expect(invokeMock).toHaveBeenLastCalledWith("render_page_region", {
      documentId: "doc-1",
      pageIndex: 2,
      scale: 20,
      x: 4000,
      y: 3000,
      width: 800,
      height: 600,
    });
  });

  it("enters and exits fullscreen through the current window", async () => {
    const setFullscreenMock = vi.fn().mockResolvedValue(undefined);
    const isFullscreenMock = vi.fn().mockResolvedValue(true);
    vi.mocked(getCurrentWindow).mockReturnValue({
      setFullscreen: setFullscreenMock,
      isFullscreen: isFullscreenMock,
    } as unknown as ReturnType<typeof getCurrentWindow>);

    await setFullscreen(true);
    await expect(isFullscreen()).resolves.toBe(true);

    expect(setFullscreenMock).toHaveBeenCalledWith(true);
    expect(isFullscreenMock).toHaveBeenCalled();
  });

  it("extracts a positioned text layer for one page", async () => {
    invokeMock.mockResolvedValue({
      pageIndex: 2,
      pageWidth: 612,
      pageHeight: 792,
      rotation: 0,
      cropX: 0,
      cropY: 0,
      hasText: true,
      text: "Hi",
      chars: [
        {
          text: "H",
          x: 72,
          y: 55.71,
          width: 13,
          height: 20,
          originX: 72,
          originY: 72,
          fontSize: 18,
          generated: false,
        },
        {
          text: "i",
          x: 85,
          y: 55.71,
          width: 5,
          height: 20,
          originX: 85,
          originY: 72,
          fontSize: 18,
          generated: false,
        },
      ],
    });

    const layer = await extractTextLayer("doc-1", 2);

    expect(invokeMock).toHaveBeenCalledWith("extract_text_layer", {
      documentId: "doc-1",
      pageIndex: 2,
    });
    expect(layer.chars[0].fontSize).toBe(18);
    expect(layer.chars[1].text).toBe("i");
    expect(layer.hasText).toBe(true);
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

  it("recognizes the save pipeline error codes", () => {
    for (const code of [
      "save_failed",
      "save_target_is_source",
      "no_output_document",
      "unencrypted_output_confirmation_required",
      "read_only_document",
    ] as const) {
      expect(toCommandError({ code, message: "detail" })).toEqual({
        code,
        message: "detail",
      });
    }
  });

  it("asks the save dialog for a PDF, preselecting the document name", async () => {
    vi.mocked(saveDialog).mockResolvedValue("/tmp/copy.pdf");

    await expect(chooseSaveFile("report.pdf")).resolves.toBe("/tmp/copy.pdf");
    expect(saveDialog).toHaveBeenCalledWith({
      defaultPath: "report.pdf",
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
  });

  it("returns null when the save dialog is cancelled", async () => {
    vi.mocked(saveDialog).mockResolvedValue(null);

    await expect(chooseSaveFile()).resolves.toBeNull();
    expect(saveDialog).toHaveBeenCalledWith({
      defaultPath: undefined,
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
  });

  it("sends the save request under the save_document command", async () => {
    const result = {
      documentId: "doc-1",
      outputPath: "/tmp/copy.pdf",
      kind: "save_as",
      bytesWritten: 12,
      backupPath: null,
      history: {
        canUndo: true,
        canRedo: false,
        revision: 1,
        savedRevision: 1,
        dirty: false,
      },
    };
    invokeMock.mockResolvedValue(result);

    await expect(
      saveDocument("doc-1", {
        saveAsPath: "/tmp/copy.pdf",
        confirmUnencryptedOutput: false,
      }),
    ).resolves.toEqual(result);
    expect(invokeMock).toHaveBeenCalledWith("save_document", {
      documentId: "doc-1",
      request: {
        saveAsPath: "/tmp/copy.pdf",
        confirmUnencryptedOutput: false,
      },
    });
  });

  it("subscribes to save progress and forwards the payload", async () => {
    const unlisten = vi.fn();
    let handler: ((event: { payload: unknown }) => void) | undefined;
    vi.mocked(listen).mockImplementation(async (event, callback) => {
      expect(event).toBe(SAVE_PROGRESS_EVENT);
      handler = callback as (event: { payload: unknown }) => void;
      return unlisten;
    });
    const received: unknown[] = [];

    const dispose = await onSaveProgress((progress) => received.push(progress));
    handler?.({
      payload: {
        documentId: "doc-1",
        stage: "writing",
        completed: 3,
        total: 4,
      },
    });

    expect(received).toEqual([
      { documentId: "doc-1", stage: "writing", completed: 3, total: 4 },
    ]);
    dispose();
    expect(unlisten).toHaveBeenCalled();
  });
});
