import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  choosePageImageDestination,
  choosePdfFile,
  chooseSaveFile,
  closeDocument,
  createFormField,
  createFormTextField,
  deleteFormField,
  deleteFormTextField,
  destroyWindow,
  exportFlattenedCopy,
  exportPage,
  extractTextLayer,
  fillFormField,
  formFields,
  initialDocumentPath,
  isFullscreen,
  MENU_EVENT,
  missingRequiredFields,
  onFileDrop,
  onMenuEvent,
  onSaveProgress,
  openDocument,
  openDocumentInNewWindow,
  pageInfo,
  printDocument,
  renderPage,
  renderPageRegion,
  renderThumbnail,
  SAVE_PROGRESS_EVENT,
  saveDocument,
  setFullscreen,
  setWindowTitle,
  toCommandError,
  updateFormField,
  updateFormTextField,
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

  it("maps page export and print to their Tauri commands", async () => {
    const exported = {
      path: "/tmp/report-page-2.png",
      format: "png",
      dpi: 300,
      width: 2550,
      height: 3300,
      bytes: 1234,
    };
    invokeMock.mockResolvedValue(exported);

    await expect(
      exportPage("doc-1", 1, "png", 300, "/tmp/report-page-2.png"),
    ).resolves.toEqual(exported);
    expect(invokeMock).toHaveBeenCalledWith("export_page", {
      documentId: "doc-1",
      pageIndex: 1,
      format: "png",
      dpi: 300,
      destination: "/tmp/report-page-2.png",
    });

    await printDocument("doc-1");
    expect(invokeMock).toHaveBeenCalledWith("print_document", {
      documentId: "doc-1",
    });
  });

  it("asks the save dialog for the selected image format", async () => {
    vi.mocked(saveDialog).mockResolvedValue("/tmp/report-page-2.jpg");

    await expect(
      choosePageImageDestination("/tmp/report-page-2.jpg", "jpg"),
    ).resolves.toBe("/tmp/report-page-2.jpg");
    expect(saveDialog).toHaveBeenCalledWith({
      defaultPath: "/tmp/report-page-2.jpg",
      filters: [{ name: "JPG", extensions: ["jpg"] }],
    });

    vi.mocked(saveDialog).mockResolvedValue(null);
    await expect(
      choosePageImageDestination("/tmp/report-page-2.png", "png"),
    ).resolves.toBeNull();
  });

  it("recognizes the export and print error codes", () => {
    expect(
      toCommandError({ code: "file_write_failed", message: "disk full" }),
    ).toEqual({ code: "file_write_failed", message: "disk full" });
    expect(
      toCommandError({ code: "print_failed", message: "no printer" }),
    ).toEqual({ code: "print_failed", message: "no printer" });
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

  it("destroys the current window after the close guard releases the document", async () => {
    const destroy = vi.fn().mockResolvedValue(undefined);
    vi.mocked(getCurrentWindow).mockReturnValue({
      destroy,
    } as unknown as ReturnType<typeof getCurrentWindow>);

    await destroyWindow();

    expect(destroy).toHaveBeenCalledTimes(1);
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

  it("recognizes the page export target error codes", () => {
    for (const code of [
      "export_target_is_source",
      "export_target_is_output",
    ] as const) {
      expect(toCommandError({ code, message: "detail" })).toEqual({
        code,
        message: "detail",
      });
    }
  });

  it("recognizes the flattened-copy export error code", () => {
    expect(
      toCommandError({ code: "flatten_export_failed", message: "detail" }),
    ).toEqual({ code: "flatten_export_failed", message: "detail" });
  });

  it("recognizes the form error codes", () => {
    for (const code of [
      "form_field_not_found",
      "form_field_read_only",
      "form_fill_not_permitted",
      "form_value_invalid",
      "form_field_name_invalid",
      "form_field_name_duplicate",
    ] as const) {
      expect(toCommandError({ code, message: "detail" })).toEqual({
        code,
        message: "detail",
      });
    }
  });

  it("maps form commands to their Tauri command names and camelCase arguments", async () => {
    const field = {
      fieldId: "0:1",
      name: "full_name",
      kind: "text",
      pageIndex: 0,
      order: 0,
      rect: { x: 72, y: 72, width: 180, height: 18 },
      value: null,
      checked: false,
      options: [],
      selectedIndex: null,
      required: true,
      readOnly: false,
      multiline: false,
      radioGroup: null,
      exportValue: null,
    };

    invokeMock.mockResolvedValue([field]);
    await expect(formFields("doc-1")).resolves.toEqual([field]);
    expect(invokeMock).toHaveBeenCalledWith("form_fields", {
      documentId: "doc-1",
    });

    invokeMock.mockResolvedValue([field]);
    await expect(missingRequiredFields("doc-1")).resolves.toEqual([field]);
    expect(invokeMock).toHaveBeenCalledWith("missing_required_fields", {
      documentId: "doc-1",
    });

    const result = {
      fieldId: "0:1",
      fields: [field],
      history: {
        canUndo: true,
        canRedo: false,
        revision: 1,
        savedRevision: 0,
        dirty: true,
      },
    };
    invokeMock.mockResolvedValue(result);
    await expect(
      fillFormField("doc-1", "0:1", { kind: "text", text: "Ada" }),
    ).resolves.toEqual(result);
    expect(invokeMock).toHaveBeenCalledWith("fill_form_field", {
      documentId: "doc-1",
      fieldId: "0:1",
      value: { kind: "text", text: "Ada" },
    });

    const createRequest = {
      rect: { x: 72, y: 100, width: 180, height: 24 },
      name: null,
      tooltip: null,
      required: false,
      multiline: false,
      fontSize: 12,
    };
    const createResult = {
      fieldId: "new:1",
      fields: [field],
      history: result.history,
    };
    invokeMock.mockResolvedValue(createResult);
    await expect(
      createFormTextField("doc-1", 0, createRequest),
    ).resolves.toEqual(createResult);
    expect(invokeMock).toHaveBeenCalledWith("create_form_text_field", {
      documentId: "doc-1",
      pageIndex: 0,
      request: createRequest,
    });

    const updateRequest = {
      name: "customer_name",
      tooltip: "请输入客户姓名",
      required: true,
      multiline: false,
      fontSize: 16,
    };
    const structureResult = { fields: [field], history: result.history };
    invokeMock.mockResolvedValue(structureResult);
    await expect(
      updateFormTextField("doc-1", "new:1", updateRequest),
    ).resolves.toEqual(structureResult);
    expect(invokeMock).toHaveBeenCalledWith("update_form_text_field", {
      documentId: "doc-1",
      fieldId: "new:1",
      request: updateRequest,
    });

    invokeMock.mockResolvedValue(structureResult);
    await expect(
      deleteFormTextField("doc-1", "new:1"),
    ).resolves.toEqual(structureResult);
    expect(invokeMock).toHaveBeenCalledWith("delete_form_text_field", {
      documentId: "doc-1",
      fieldId: "new:1",
    });
  });

  it("maps checkbox and radio field commands to the shared generic seam", async () => {
    const history = {
      canUndo: true,
      canRedo: false,
      revision: 1,
      savedRevision: 0,
      dirty: true,
    };
    const field = {
      fieldId: "new:2",
      name: "terms",
      kind: "checkbox",
      pageIndex: 0,
      order: 0,
      rect: { x: 72, y: 100, width: 16, height: 16 },
      value: "Off",
      checked: false,
      options: [],
      selectedIndex: null,
      required: false,
      readOnly: false,
      multiline: false,
      radioGroup: null,
      exportValue: "Yes",
    };
    const createResult = { fieldId: "new:2", fields: [field], history };
    const checkboxRequest = {
      kind: "checkbox" as const,
      rect: { x: 72, y: 100, width: 16, height: 16 },
      name: null,
      tooltip: null,
      required: false,
      checked: false,
    };
    invokeMock.mockResolvedValue(createResult);
    await expect(
      createFormField("doc-1", 0, checkboxRequest),
    ).resolves.toEqual(createResult);
    expect(invokeMock).toHaveBeenLastCalledWith("create_form_field", {
      documentId: "doc-1",
      pageIndex: 0,
      request: checkboxRequest,
    });

    const radioRequest = {
      kind: "radio" as const,
      name: "gender",
      tooltip: "请选择性别",
      required: true,
      options: ["男", "女"],
      selectedIndex: 1,
    };
    const structureResult = { fields: [field], history };
    invokeMock.mockResolvedValue(structureResult);
    await expect(
      updateFormField("doc-1", "new:2", radioRequest),
    ).resolves.toEqual(structureResult);
    expect(invokeMock).toHaveBeenLastCalledWith("update_form_field", {
      documentId: "doc-1",
      fieldId: "new:2",
      request: radioRequest,
    });

    invokeMock.mockResolvedValue(structureResult);
    await expect(deleteFormField("doc-1", "new:2")).resolves.toEqual(
      structureResult,
    );
    expect(invokeMock).toHaveBeenLastCalledWith("delete_form_field", {
      documentId: "doc-1",
      fieldId: "new:2",
    });
  });

  it("maps choice and signature requests to the shared generic seam", async () => {
    const history = {
      canUndo: true,
      canRedo: false,
      revision: 1,
      savedRevision: 0,
      dirty: true,
    };
    const field = {
      fieldId: "new:3",
      name: "signature_1",
      kind: "signature",
      pageIndex: 0,
      order: 0,
      rect: { x: 72, y: 100, width: 240, height: 60 },
      value: null,
      checked: false,
      options: [],
      selectedIndex: null,
      selectedIndices: [],
      multiSelect: false,
      required: false,
      readOnly: false,
      multiline: false,
      radioGroup: null,
      exportValue: null,
    };
    const createResult = { fieldId: "new:3", fields: [field], history };

    const comboRequest = {
      kind: "comboBox" as const,
      rect: { x: 72, y: 100, width: 180, height: 24 },
      name: null,
      tooltip: null,
      required: false,
      options: ["中国", "美国"],
      selectedIndex: 1,
      fontSize: 12,
    };
    invokeMock.mockResolvedValue(createResult);
    await expect(createFormField("doc-1", 0, comboRequest)).resolves.toEqual(
      createResult,
    );
    expect(invokeMock).toHaveBeenLastCalledWith("create_form_field", {
      documentId: "doc-1",
      pageIndex: 0,
      request: comboRequest,
    });

    const structureResult = { fields: [field], history };
    const listRequest = {
      kind: "listBox" as const,
      name: "cities",
      tooltip: null,
      required: false,
      options: ["北京", "上海"],
      selectedIndices: [0, 1],
      multiSelect: true,
      fontSize: 10,
    };
    invokeMock.mockResolvedValue(structureResult);
    await expect(
      updateFormField("doc-1", "new:3", listRequest),
    ).resolves.toEqual(structureResult);
    expect(invokeMock).toHaveBeenLastCalledWith("update_form_field", {
      documentId: "doc-1",
      fieldId: "new:3",
      request: listRequest,
    });

    const signatureRequest = {
      kind: "signature" as const,
      name: "sign_here",
      tooltip: "请在此签名",
    };
    invokeMock.mockResolvedValue(structureResult);
    await expect(
      updateFormField("doc-1", "new:3", signatureRequest),
    ).resolves.toEqual(structureResult);
    expect(invokeMock).toHaveBeenLastCalledWith("update_form_field", {
      documentId: "doc-1",
      fieldId: "new:3",
      request: signatureRequest,
    });
  });

  it("sends a flattened-copy export under the export_flattened_copy command", async () => {
    const result = {
      documentId: "doc-1",
      path: "/tmp/report-flattened.pdf",
      bytes: 4096,
      flattened: 3,
      preserved: 2,
      skipped: [
        { pageIndex: 0, subtype: "Square", reason: "no_appearance" },
      ],
    };
    invokeMock.mockResolvedValue(result);

    await expect(
      exportFlattenedCopy("doc-1", "/tmp/report-flattened.pdf", true),
    ).resolves.toEqual(result);
    expect(invokeMock).toHaveBeenCalledWith("export_flattened_copy", {
      documentId: "doc-1",
      destination: "/tmp/report-flattened.pdf",
      confirmUnencryptedOutput: true,
    });
  });

  it("defaults the flattened-copy unencrypted confirmation to false", async () => {
    invokeMock.mockResolvedValue({
      documentId: "doc-1",
      path: "/tmp/report-flattened.pdf",
      bytes: 0,
      flattened: 0,
      preserved: 0,
      skipped: [],
    });

    await exportFlattenedCopy("doc-1", "/tmp/report-flattened.pdf");
    expect(invokeMock).toHaveBeenCalledWith("export_flattened_copy", {
      documentId: "doc-1",
      destination: "/tmp/report-flattened.pdf",
      confirmUnencryptedOutput: false,
    });
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
