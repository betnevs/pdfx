import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";
import {
  READ_ONLY_REASON,
  UNENCRYPTED_OUTPUT_WARNING,
} from "./lib/document-policy";
import { notifyDocumentChanged } from "./lib/documentEvents";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const openDialogMock = vi.mocked(openDialog);
const saveDialogMock = vi.mocked(saveDialog);
const listenMock = vi.mocked(listen);

const allGrantedPermissions = {
  canModifyContent: true,
  canAnnotate: true,
  canAssemble: true,
  canFillForms: true,
  canPrint: true,
  canCopyContent: true,
};

const documentView = {
  documentId: "doc-1",
  name: "report.pdf",
  path: "/tmp/report.pdf",
  pageCount: 3,
  pageWidth: 612,
  pageHeight: 792,
  encrypted: false,
  readOnly: false,
  permissions: allGrantedPermissions,
};

const idleHistory = {
  canUndo: false,
  canRedo: false,
  revision: 0,
  savedRevision: 0,
  dirty: false,
};

const dirtyHistory = {
  canUndo: true,
  canRedo: false,
  revision: 1,
  savedRevision: 0,
  dirty: true,
};

const cleanHistory = {
  canUndo: true,
  canRedo: false,
  revision: 2,
  savedRevision: 2,
  dirty: false,
};

function savedResult(overrides: Record<string, unknown> = {}) {
  return {
    documentId: "doc-1",
    outputPath: "/tmp/copy.pdf",
    kind: "save_as",
    bytesWritten: 2048,
    backupPath: null,
    history: cleanHistory,
    ...overrides,
  };
}

const pageImage = {
  documentId: "doc-1",
  pageIndex: 0,
  scale: 1,
  width: 612,
  height: 792,
  pngBase64: "iVBORw0KGgo=",
};

function defaultPageInfo(pageIndex: number) {
  return {
    pageIndex,
    width: 612,
    height: 792,
    rotation: 0,
    cropX: 0,
    cropY: 0,
  };
}

function thumbnailImage(args: { pageIndex: number; documentId?: string }) {
  return {
    documentId: args.documentId ?? "doc-1",
    pageIndex: args.pageIndex,
    scale: 0.15,
    width: 93,
    height: 120,
    pngBase64: "iVBORw0KGgo=",
  };
}

const textLayerView = {
  pageIndex: 0,
  pageWidth: 612,
  pageHeight: 792,
  rotation: 0,
  cropX: 0,
  cropY: 0,
  hasText: true,
  text: "PDFX fixture",
  chars: [
    {
      text: "P",
      x: 72,
      y: 55.71,
      width: 12.006,
      height: 20.088,
      originX: 72,
      originY: 72,
      fontSize: 18,
      generated: false,
    },
  ],
};

type CommandHandler = (args: unknown) => unknown;

function installBackend(overrides: Record<string, CommandHandler> = {}) {
  invokeMock.mockImplementation(async (command: string, args?: unknown) => {
    if (command === "app_info") {
      return { name: "PDFX", version: "0.1.0" };
    }
    if (command === "get_settings") {
      return {
        language: "zh-CN",
        theme: "system",
        annotationAuthor: "PDFX 用户",
        fileAssociationEnabled: false,
        updateManifestUrl: "",
      };
    }
    const handler = overrides[command];
    if (handler) {
      return handler(args);
    }
    throw new Error(`unexpected command: ${command}`);
  });
}

function installDocumentBackend(overrides: Record<string, CommandHandler> = {}) {
  installBackend({
    open_document: () => documentView,
    page_info: (args) => defaultPageInfo((args as { pageIndex: number }).pageIndex),
    render_page: (args) => ({
      ...pageImage,
      pageIndex: (args as { pageIndex: number }).pageIndex,
    }),
    render_thumbnail: (args) =>
      thumbnailImage(args as { pageIndex: number; documentId?: string }),
    render_page_region: (args) => {
      const region = args as {
        documentId: string;
        pageIndex: number;
        scale: number;
        x: number;
        y: number;
        width: number;
        height: number;
      };
      return { ...region, pngBase64: "iVBORw0KGgo=" };
    },
    extract_text_layer: () => textLayerView,
    form_fields: () => [],
    missing_required_fields: () => [],
    page_images: () => [],
    page_line_objects: () => [],
    edit_history: () => idleHistory,
    list_annotations: () => [],
    list_links: () => [],
    close_document: () => undefined,
    set_menu_state: () => undefined,
    ...overrides,
  });
}

function renderPageCalls(): Array<{ documentId: string; pageIndex: number; scale: number }> {
  return invokeMock.mock.calls
    .filter(([command]) => command === "render_page")
    .map(([, args]) => args as { documentId: string; pageIndex: number; scale: number });
}

function renderRegionCalls() {
  return invokeMock.mock.calls
    .filter(([command]) => command === "render_page_region")
    .map(
      ([, args]) =>
        args as {
          documentId: string;
          pageIndex: number;
          scale: number;
          x: number;
          y: number;
          width: number;
          height: number;
        },
    );
}

function thumbnailCalls(): Array<{ documentId: string; pageIndex: number; maxEdge: number }> {
  return invokeMock.mock.calls
    .filter(([command]) => command === "render_thumbnail")
    .map(([, args]) => args as { documentId: string; pageIndex: number; maxEdge: number });
}

function pageInfoCalls(): Array<{ documentId: string; pageIndex: number }> {
  return invokeMock.mock.calls
    .filter(([command]) => command === "page_info")
    .map(([, args]) => args as { documentId: string; pageIndex: number });
}

function commandCalls(command: string) {
  return invokeMock.mock.calls.filter(([name]) => name === command);
}

/** The enabled flags from the latest snapshot pushed to the native menu. */
function lastMenuState(): Map<string, boolean> {
  const call = commandCalls("set_menu_state").at(-1);
  if (!call) {
    throw new Error("no menu state has been pushed");
  }
  const { state } = call[1] as {
    state: { items: Array<{ id: string; enabled: boolean }> };
  };
  return new Map(state.items.map((item) => [item.id, item.enabled]));
}

function setViewportSize(width: number, height: number) {
  const viewport = screen.getByLabelText("页面视口");
  Object.defineProperty(viewport, "clientWidth", { configurable: true, value: width });
  Object.defineProperty(viewport, "clientHeight", { configurable: true, value: height });
}

type CloseHandler = (event: { preventDefault: () => void }) => Promise<void>;

/** The guard the app registered for the current window's close event. */
function windowCloseHandler(): CloseHandler {
  return onCloseRequested.mock.calls[0][0] as CloseHandler;
}

/** Delivers one OS close request and waits for the guard to decide. */
async function triggerWindowClose(): Promise<ReturnType<typeof vi.fn>> {
  await waitFor(() => expect(onCloseRequested).toHaveBeenCalled());
  const preventDefault = vi.fn();
  await act(async () => {
    await windowCloseHandler()({ preventDefault });
  });
  return preventDefault;
}

function closeConfirmDialog(): HTMLElement {
  return screen.getByRole("dialog", { name: "关闭前保存更改？" });
}

let setTitle: ReturnType<typeof vi.fn>;
let onCloseRequested: ReturnType<typeof vi.fn>;
let destroyWindowMock: ReturnType<typeof vi.fn>;
let onDragDropEvent: ReturnType<typeof vi.fn>;
let setFullscreenMock: ReturnType<typeof vi.fn>;
let isFullscreenMock: ReturnType<typeof vi.fn>;
let dragDropHandler: ((event: unknown) => void) | undefined;
let menuHandler: ((event: unknown) => void) | undefined;
let saveProgressHandler: ((event: { payload: unknown }) => void) | undefined;

beforeEach(() => {
  setTitle = vi.fn().mockResolvedValue(undefined);
  onCloseRequested = vi.fn().mockResolvedValue(() => {});
  destroyWindowMock = vi.fn().mockResolvedValue(undefined);
  setFullscreenMock = vi.fn().mockResolvedValue(undefined);
  isFullscreenMock = vi.fn().mockResolvedValue(false);
  dragDropHandler = undefined;
  menuHandler = undefined;
  saveProgressHandler = undefined;
  onDragDropEvent = vi.fn(async (handler: (event: unknown) => void) => {
    dragDropHandler = handler;
    return () => {};
  });

  vi.mocked(getCurrentWindow).mockReturnValue({
    setTitle,
    onCloseRequested,
    destroy: destroyWindowMock,
    setFullscreen: setFullscreenMock,
    isFullscreen: isFullscreenMock,
  } as unknown as ReturnType<typeof getCurrentWindow>);
  vi.mocked(getCurrentWebview).mockReturnValue({
    onDragDropEvent,
  } as unknown as ReturnType<typeof getCurrentWebview>);
  listenMock.mockImplementation(async (event, handler) => {
    if (event === "pdfx://menu") {
      menuHandler = handler as (event: unknown) => void;
    } else if (event === "pdfx://save-progress") {
      saveProgressHandler = handler as (event: { payload: unknown }) => void;
    }
    return () => {};
  });
  openDialogMock.mockResolvedValue(null);
  saveDialogMock.mockResolvedValue(null);
  window.history.replaceState({}, "", "/");
});

afterEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(getCurrentWindow).mockReset();
  vi.mocked(getCurrentWebview).mockReset();
  listenMock.mockReset();
  openDialogMock.mockReset();
  saveDialogMock.mockReset();
  window.history.replaceState({}, "", "/");
});

describe("PDFX workspace", () => {
  it("shows the empty workspace and the app version from the backend", async () => {
    vi.mocked(invoke).mockResolvedValue({ name: "PDFX", version: "9.8.7" });

    render(<App />);

    expect(screen.getByRole("heading", { level: 1, name: "PDFX" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 2, name: "空工作区" })).toBeInTheDocument();
    expect(
      screen.getByText("打开一个 PDF 文件开始编辑。文件只在本机处理。"),
    ).toBeInTheDocument();
    expect(await screen.findByRole("status")).toHaveTextContent("PDFX 9.8.7");
  });

  it("keeps the empty workspace visible when app info is unavailable", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("not running inside Tauri"));

    render(<App />);

    expect(await screen.findByRole("status")).toHaveTextContent("无法获取应用信息");
    expect(screen.getByRole("heading", { level: 2, name: "空工作区" })).toBeInTheDocument();
  });

  it("opens a PDF from the file dialog, renders page one and titles the window", async () => {
    installDocumentBackend();
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));

    await waitFor(() => {
      expect(commandCalls("open_document")).toEqual([
        ["open_document", { path: "/tmp/report.pdf", password: null }],
      ]);
    });
    await waitFor(() => {
      expect(renderPageCalls()).toEqual([
        { documentId: "doc-1", pageIndex: 0, scale: 1 },
      ]);
    });

    expect(await screen.findByText("report.pdf")).toBeInTheDocument();
    expect(setTitle).toHaveBeenCalledWith("report.pdf — PDFX");
    expect(screen.getByRole("img", { name: "第 1 页" })).toHaveStyle({
      width: "612px",
      height: "792px",
    });
    expect(screen.getByText("第 1 / 3 页 · 100%")).toBeInTheDocument();
    expect(openDialogMock).toHaveBeenCalledWith(
      expect.objectContaining({
        multiple: false,
        filters: [{ name: "PDF", extensions: ["pdf"] }],
      }),
    );
  });

  it("loads the visible page's text layer alongside the rendered canvas", async () => {
    installDocumentBackend();
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));

    expect(await screen.findByRole("region", { name: "文本层" })).toBeInTheDocument();
    expect(commandCalls("extract_text_layer")).toEqual([
      ["extract_text_layer", { documentId: "doc-1", pageIndex: 0 }],
    ]);
    expect(screen.getByRole("img", { name: "第 1 页" })).toBeInTheDocument();
  });

  it("opens a PDF dropped onto the window", async () => {
    installDocumentBackend();
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await waitFor(() => expect(dragDropHandler).toBeDefined());

    act(() => {
      dragDropHandler?.({
        payload: { type: "drop", paths: ["/tmp/dropped.pdf"], position: { x: 0, y: 0 } },
      });
    });

    await waitFor(() => {
      expect(commandCalls("open_document")).toEqual([
        ["open_document", { path: "/tmp/dropped.pdf", password: null }],
      ]);
    });
    // Wait for the opened document's effects so the save listener is
    // registered inside this test rather than after it tears its mocks down.
    expect(await screen.findByText("report.pdf")).toBeInTheDocument();
  });

  it("jumps to a listed annotation's page, outlines it and deletes it", async () => {
    let annotations = [
      {
        name: "pdfx-ann-1",
        pageIndex: 2,
        kind: "highlight" as const,
        author: "测试作者",
        createdAt: "D:20261028123456Z",
        color: "#ffd400",
        opacity: 50,
        rects: [{ x: 100, y: 100, width: 12, height: 20 }],
        shape: null,
        stamp: null,
        freeText: null,
      },
    ];
    installDocumentBackend({
      list_annotations: () => annotations,
      delete_annotation: () => {
        annotations = [];
        return {
          annotation: null,
          pageIndex: 2,
          history: {
            canUndo: true,
            canRedo: false,
            revision: 1,
            savedRevision: 0,
            dirty: true,
          },
        };
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    const item = await screen.findByRole("button", {
      name: "高亮，第 3 页，测试作者",
    });
    expect(screen.getByRole("heading", { name: "第 3 页" })).toBeInTheDocument();

    await user.click(item);
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({ pageIndex: 2 });
    });
    expect(
      await screen.findByTestId("selected-annotation-outline"),
    ).toBeInTheDocument();
    expect(screen.getByText("2026-10-28 12:34 UTC")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "删除注释" }));

    await waitFor(() => {
      expect(commandCalls("delete_annotation")).toEqual([
        ["delete_annotation", { documentId: "doc-1", name: "pdfx-ann-1" }],
      ]);
    });
    expect(await screen.findByText("还没有注释")).toBeInTheDocument();
    expect(
      screen.queryByTestId("selected-annotation-outline"),
    ).not.toBeInTheDocument();
  });

  it("places a stamp from the panel on the page and selects it", async () => {
    let annotations: unknown[] = [];
    const placed = {
      name: "pdfx-stamp-1",
      pageIndex: 0,
      kind: "stamp" as const,
      author: "PDFX 用户",
      createdAt: "D:20261028123456Z",
      color: "#15803d",
      opacity: 100,
      rects: [{ x: 216, y: 366, width: 180, height: 60 }],
      stamp: {
        kind: "approved" as const,
        frame: { x: 216, y: 366, width: 180, height: 60, rotation: 0 },
      },
    };
    installDocumentBackend({
      list_annotations: () => annotations,
      create_stamp_annotation: () => {
        annotations = [placed];
        return {
          annotation: placed,
          pageIndex: 0,
          history: {
            canUndo: true,
            canRedo: false,
            revision: 1,
            savedRevision: 0,
            dirty: true,
          },
        };
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await user.click(screen.getByRole("button", { name: "放置已批准图章" }));
    expect(
      screen.getByText("在页面上单击放置“已批准”图章。"),
    ).toBeInTheDocument();

    const layer = await screen.findByTestId("stamp-layer");
    vi.spyOn(layer, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      left: 0,
      top: 0,
      right: 612,
      bottom: 792,
      width: 612,
      height: 792,
      toJSON: () => ({}),
    } as DOMRect);
    fireEvent.pointerDown(layer, {
      button: 0,
      clientX: 306,
      clientY: 396,
      pointerId: 9,
    });

    await waitFor(() => {
      expect(commandCalls("create_stamp_annotation")).toEqual([
        [
          "create_stamp_annotation",
          {
            documentId: "doc-1",
            request: {
              pageIndex: 0,
              kind: "approved",
              opacity: 100,
              frame: { x: 216, y: 366, width: 180, height: 60, rotation: 0 },
            },
          },
        ],
      ]);
    });
    expect(
      await screen.findByRole("button", {
        name: "图章 · 已批准，第 1 页，PDFX 用户",
      }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText("图章：已批准")).toBeInTheDocument();
    expect(
      screen.queryByText("在页面上单击放置“已批准”图章。"),
    ).not.toBeInTheDocument();
  });

  it("opens a second file in its own window instead of replacing the first document", async () => {
    installDocumentBackend({ open_document_window: () => undefined });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf").mockResolvedValueOnce("/tmp/other.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));

    await waitFor(() => {
      expect(commandCalls("open_document_window")).toEqual([
        ["open_document_window", { path: "/tmp/other.pdf" }],
      ]);
    });
    expect(commandCalls("open_document")).toHaveLength(1);
    expect(screen.getByText("report.pdf")).toBeInTheDocument();
    expect(setTitle).toHaveBeenCalledTimes(1);
  });

  it("keeps the current document when creating a second window fails", async () => {
    installDocumentBackend({
      open_document_window: () => {
        throw { code: "window_failed", message: "could not create a document window" };
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf").mockResolvedValueOnce("/tmp/other.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("无法打开新窗口。");
    expect(screen.getByText("report.pdf")).toBeInTheDocument();
    expect(setTitle).toHaveBeenCalledTimes(1);
    expect(commandCalls("open_document")).toHaveLength(1);
  });

  it("shows a clear error with a retry action for a corrupt PDF", async () => {
    installBackend({
      open_document: () => {
        throw { code: "corrupt_document", message: "PDFium could not open the document" };
      },
    });
    openDialogMock.mockResolvedValue("/tmp/broken.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("文件已损坏或格式不受支持。");
    expect(alert).toHaveTextContent("PDFium could not open the document");

    await user.click(screen.getByRole("button", { name: "重试" }));

    await waitFor(() => expect(commandCalls("open_document")).toHaveLength(2));
  });

  it("asks for a password, rejects a wrong one without rendering, then opens with the right one", async () => {
    installDocumentBackend({
      open_document: (args) => {
        const { password } = args as { password: string | null };
        if (password === null) {
          throw { code: "password_required", message: "this document requires a password" };
        }
        if (password !== "pdfx-user") {
          throw {
            code: "invalid_password",
            message: "the supplied password did not open the document",
          };
        }
        return {
          ...documentView,
          name: "secret.pdf",
          path: "/tmp/secret.pdf",
          encrypted: true,
        };
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/secret.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));

    expect(await screen.findByRole("dialog", { name: "输入密码" })).toBeInTheDocument();
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
    expect(renderPageCalls()).toEqual([]);
    expect(commandCalls("open_document")).toEqual([
      ["open_document", { path: "/tmp/secret.pdf", password: null }],
    ]);

    await user.type(screen.getByLabelText("密码"), "wrong");
    await user.click(screen.getByRole("button", { name: "打开" }));

    expect(await screen.findByText("密码不正确，请重试。")).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "输入密码" })).toBeInTheDocument();
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
    expect(renderPageCalls()).toEqual([]);

    await user.clear(screen.getByLabelText("密码"));
    await user.type(screen.getByLabelText("密码"), "pdfx-user");
    await user.click(screen.getByRole("button", { name: "打开" }));

    expect(await screen.findByText("secret.pdf")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await waitFor(() => {
      expect(renderPageCalls()).toEqual([
        { documentId: "doc-1", pageIndex: 0, scale: 1 },
      ]);
    });
    expect(commandCalls("open_document").at(-1)).toEqual([
      "open_document",
      { path: "/tmp/secret.pdf", password: "pdfx-user" },
    ]);
  });

  it("returns to the empty workspace when the password prompt is cancelled", async () => {
    installDocumentBackend({
      open_document: () => {
        throw { code: "password_required", message: "this document requires a password" };
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/secret.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByRole("dialog", { name: "输入密码" });

    await user.click(screen.getByRole("button", { name: "取消" }));

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 2, name: "空工作区" })).toBeInTheDocument();
    expect(renderPageCalls()).toEqual([]);
  });

  it("shows the read-only reason and disables editing and save entries for restricted documents", async () => {
    installDocumentBackend({
      open_document: () => ({
        ...documentView,
        encrypted: true,
        readOnly: true,
        permissions: {
          ...allGrantedPermissions,
          canModifyContent: false,
          canAnnotate: false,
          canAssemble: false,
          canFillForms: false,
          canPrint: true,
          canCopyContent: false,
        },
      }),
    });
    openDialogMock.mockResolvedValueOnce("/tmp/restricted.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));

    expect(await screen.findByText(READ_ONLY_REASON)).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "第 1 页" })).toBeInTheDocument();

    const editing = screen.getByRole("button", { name: "编辑工具（即将提供）" });
    expect(editing).toBeDisabled();
    expect(editing).toHaveAttribute("aria-describedby", "read-only-reason");

    expect(screen.getByRole("button", { name: "保存更改" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "另存为…" })).toBeEnabled();
    expect(screen.getByText(UNENCRYPTED_OUTPUT_WARNING)).toBeInTheDocument();
  });

  it("does not report an editable encrypted document as read-only but warns about unencrypted output", async () => {
    installDocumentBackend({
      open_document: () => ({ ...documentView, encrypted: true }),
    });
    openDialogMock.mockResolvedValueOnce("/tmp/encrypted.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));

    expect(await screen.findByRole("img", { name: "第 1 页" })).toBeInTheDocument();
    expect(screen.queryByText(READ_ONLY_REASON)).not.toBeInTheDocument();
    expect(screen.getByText(UNENCRYPTED_OUTPUT_WARNING)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "保存更改" })).toBeEnabled();
  });

  it("shows existing form fields on the page and fills them through the backend", async () => {
    const formField = {
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
      required: false,
      readOnly: false,
      multiline: false,
      radioGroup: null,
      exportValue: null,
    };
    installDocumentBackend({
      form_fields: () => [formField],
      fill_form_field: (args) => {
        expect(args).toEqual({
          documentId: "doc-1",
          fieldId: "0:1",
          value: { kind: "text", text: "Ada Lovelace" },
        });
        return {
          fieldId: "0:1",
          fields: [{ ...formField, value: "Ada Lovelace" }],
          history: {
            canUndo: true,
            canRedo: false,
            revision: 1,
            savedRevision: 0,
            dirty: true,
          },
        };
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/form.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));

    const input = await screen.findByLabelText("full_name");
    fireEvent.change(input, { target: { value: "Ada Lovelace" } });
    fireEvent.blur(input);

    await waitFor(() => expect(input).toHaveValue("Ada Lovelace"));
    expect(commandCalls("fill_form_field")).toHaveLength(1);
  });

  it("disables form fields when the document forbids filling them", async () => {
    installDocumentBackend({
      open_document: () => ({
        ...documentView,
        permissions: { ...allGrantedPermissions, canFillForms: false },
      }),
      form_fields: () => [
        {
          fieldId: "0:1",
          name: "full_name",
          kind: "text",
          pageIndex: 0,
          order: 0,
          rect: { x: 72, y: 72, width: 180, height: 18 },
          value: "Read only",
          checked: false,
          options: [],
          selectedIndex: null,
          required: false,
          readOnly: false,
          multiline: false,
          radioGroup: null,
          exportValue: null,
        },
      ],
    });
    openDialogMock.mockResolvedValueOnce("/tmp/form.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));

    expect(await screen.findByLabelText("full_name")).toBeDisabled();
    expect(screen.getByText("此文档不允许填写表单")).toBeInTheDocument();
  });

  it("blocks saving while required fields are empty and locates the first one", async () => {
    const notes = {
      fieldId: "1:1",
      name: "notes",
      kind: "text",
      pageIndex: 1,
      order: 0,
      rect: { x: 72, y: 72, width: 300, height: 60 },
      value: null,
      checked: false,
      options: [],
      selectedIndex: null,
      required: true,
      readOnly: false,
      multiline: true,
      radioGroup: null,
      exportValue: null,
    };
    installDocumentBackend({
      form_fields: () => [notes],
      missing_required_fields: () => [notes],
    });
    openDialogMock.mockResolvedValueOnce("/tmp/form.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await user.click(screen.getByRole("button", { name: "保存更改" }));

    expect(
      await screen.findByText(/有 1 个必填字段尚未填写，无法保存/),
    ).toBeInTheDocument();
    expect(screen.getByText(/第 2 页「notes」/)).toBeInTheDocument();
    expect(commandCalls("save_document")).toHaveLength(0);
    expect(saveDialogMock).not.toHaveBeenCalled();

    // The viewer navigated to the missing field's page and focused it.
    expect(await screen.findByText("第 2 / 3 页 · 100%")).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByLabelText("notes")).toHaveFocus(),
    );

    // Save As is blocked before it ever opens the target dialog.
    await user.click(screen.getByRole("button", { name: "另存为…" }));
    await waitFor(() =>
      expect(commandCalls("missing_required_fields")).toHaveLength(2),
    );
    expect(saveDialogMock).not.toHaveBeenCalled();
    expect(commandCalls("save_document")).toHaveLength(0);
  });

  it("navigates pages with the toolbar, the page input and Home/End", async () => {
    installDocumentBackend();
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await waitFor(() => {
      expect(renderPageCalls()).toEqual([
        { documentId: "doc-1", pageIndex: 0, scale: 1 },
      ]);
    });
    expect(screen.getByRole("button", { name: "上一页" })).toBeDisabled();
    expect(screen.getByText("第 1 / 3 页 · 100%")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "下一页" }));
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toEqual({
        documentId: "doc-1",
        pageIndex: 1,
        scale: 1,
      });
    });
    expect(screen.getByText("第 2 / 3 页 · 100%")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "上一页" }));
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({ pageIndex: 0 });
    });

    const pageInput = screen.getByRole("spinbutton", { name: "当前页码" });
    await user.clear(pageInput);
    await user.type(pageInput, "3{Enter}");
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({ pageIndex: 2 });
    });
    expect(pageInput).toHaveValue(3);
    expect(screen.getByText("第 3 / 3 页 · 100%")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "下一页" })).toBeDisabled();

    // Out-of-range input clamps to the last page instead of failing.
    await user.clear(pageInput);
    await user.type(pageInput, "99{Enter}");
    await waitFor(() => expect(pageInput).toHaveValue(3));
    expect(renderPageCalls().at(-1)).toMatchObject({ pageIndex: 2 });

    // Home and End jump to the first and last page from the page viewport.
    const viewport = screen.getByLabelText("页面视口");
    fireEvent.keyDown(viewport, { key: "Home" });
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({ pageIndex: 0 });
    });
    expect(screen.getByText("第 1 / 3 页 · 100%")).toBeInTheDocument();

    fireEvent.keyDown(viewport, { key: "End" });
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({ pageIndex: 2 });
    });
    expect(screen.getByText("第 3 / 3 页 · 100%")).toBeInTheDocument();

    // Page metadata is fetched once per page and then served from the cache.
    expect(pageInfoCalls().map((call) => call.pageIndex).sort()).toEqual([
      0, 1, 2,
    ]);
  });

  it("publishes menu availability for the empty workspace and the open document", async () => {
    installDocumentBackend({ edit_history: () => dirtyHistory });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);

    await waitFor(() => {
      const state = lastMenuState();
      expect(state.get("file.open")).toBe(true);
      expect(state.get("app.settings")).toBe(true);
      expect(state.get("file.save")).toBe(false);
      expect(state.get("file.save_as")).toBe(false);
      expect(state.get("file.print")).toBe(false);
      expect(state.get("edit.undo")).toBe(false);
      expect(state.get("edit.redo")).toBe(false);
      expect(state.get("view.zoom_in")).toBe(false);
      expect(state.get("page.prev")).toBe(false);
      expect(state.get("page.next")).toBe(false);
    });

    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await waitFor(() => {
      const state = lastMenuState();
      expect(state.get("file.save")).toBe(true);
      expect(state.get("file.save_as")).toBe(true);
      expect(state.get("file.print")).toBe(true);
      expect(state.get("file.export_page")).toBe(true);
      expect(state.get("edit.undo")).toBe(true);
      expect(state.get("edit.redo")).toBe(false);
      expect(state.get("view.zoom_in")).toBe(true);
      // First page: backwards is unavailable, forwards is available.
      expect(state.get("page.prev")).toBe(false);
      expect(state.get("page.next")).toBe(true);
    });

    act(() => menuHandler?.({ payload: "page.next" }));
    await waitFor(() => {
      const state = lastMenuState();
      expect(state.get("page.prev")).toBe(true);
      expect(state.get("page.next")).toBe(true);
    });
    // The toolbar input and the thumbnail selection move with the shortcut.
    const pageInput = screen.getByRole("spinbutton", { name: "当前页码" });
    expect(pageInput).toHaveValue(2);
    expect(screen.getByRole("button", { name: "第 2 页" })).toHaveAttribute(
      "aria-current",
      "page",
    );

    // The last page disables the forward item immediately.
    act(() => menuHandler?.({ payload: "page.next" }));
    await waitFor(() => {
      const state = lastMenuState();
      expect(state.get("page.prev")).toBe(true);
      expect(state.get("page.next")).toBe(false);
    });
    expect(pageInput).toHaveValue(3);
    expect(screen.getByRole("button", { name: "第 3 页" })).toHaveAttribute(
      "aria-current",
      "page",
    );

    // A past-the-end event cannot emit an invalid page command.
    act(() => menuHandler?.({ payload: "page.next" }));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(pageInput).toHaveValue(3);
    expect(screen.getByText("第 3 / 3 页 · 100%")).toBeInTheDocument();
  });

  it("runs menu undo and redo once against the unified edit history", async () => {
    const undoable = {
      canUndo: true,
      canRedo: false,
      revision: 2,
      savedRevision: 2,
      dirty: false,
    };
    const undone = {
      canUndo: false,
      canRedo: true,
      revision: 1,
      savedRevision: 2,
      dirty: true,
    };
    const redone = {
      canUndo: true,
      canRedo: false,
      revision: 2,
      savedRevision: 2,
      dirty: false,
    };
    let currentHistory = undoable;
    installDocumentBackend({
      edit_history: () => currentHistory,
      undo_text_edit: () => {
        currentHistory = undone;
        return {
          pageIndex: 1,
          pageCount: 3,
          pageStructureChanged: false,
          history: undone,
        };
      },
      redo_text_edit: () => {
        currentHistory = redone;
        return {
          pageIndex: 1,
          pageCount: 3,
          pageStructureChanged: false,
          history: redone,
        };
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await waitFor(() => {
      const state = lastMenuState();
      expect(state.get("edit.undo")).toBe(true);
      expect(state.get("edit.redo")).toBe(false);
    });

    // Cmd/Ctrl+Z is the native menu's accelerator; the webview must not also
    // handle it, or one key press would produce two history steps.
    const viewport = screen.getByLabelText("页面视口");
    const textLayer = screen.getByRole("region", { name: "文本层" });
    fireEvent.keyDown(viewport, { key: "z", ctrlKey: true });
    fireEvent.keyDown(textLayer, { key: "z", ctrlKey: true, shiftKey: true });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(commandCalls("undo_text_edit")).toHaveLength(0);
    expect(commandCalls("redo_text_edit")).toHaveLength(0);

    const rendersBefore = renderPageCalls().length;
    act(() => menuHandler?.({ payload: "edit.undo" }));
    await waitFor(() => expect(commandCalls("undo_text_edit")).toHaveLength(1));
    // The unified history change repaints the page and refreshes availability.
    await waitFor(() =>
      expect(renderPageCalls().length).toBeGreaterThan(rendersBefore),
    );
    await waitFor(() => {
      const state = lastMenuState();
      expect(state.get("edit.undo")).toBe(false);
      expect(state.get("edit.redo")).toBe(true);
    });

    act(() => menuHandler?.({ payload: "edit.redo" }));
    await waitFor(() => expect(commandCalls("redo_text_edit")).toHaveLength(1));
    await waitFor(() => {
      const state = lastMenuState();
      expect(state.get("edit.undo")).toBe(true);
      expect(state.get("edit.redo")).toBe(false);
    });
    expect(commandCalls("undo_text_edit")).toHaveLength(1);
  });

  it("dispatches menu undo after a link is created over selected text", async () => {
    let linkCreated = false;
    installDocumentBackend({
      edit_history: () => (linkCreated ? dirtyHistory : idleHistory),
      create_link: () => {
        linkCreated = true;
        return {
          link: {
            id: "0:0",
            name: "pdfx-link-1",
            pageIndex: 0,
            target: { kind: "url", url: "https://example.com" },
            unsupported: null,
            rects: [{ x: 72, y: 55.71, width: 12.006, height: 20.088 }],
          },
          pageIndex: 0,
          history: dirtyHistory,
        };
      },
      undo_text_edit: () => ({ pageIndex: 0, history: idleHistory }),
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    const textLayer = await screen.findByRole("region", { name: "文本层" });
    vi.spyOn(textLayer, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      left: 0,
      top: 0,
      right: 612,
      bottom: 792,
      width: 612,
      height: 792,
      toJSON: () => ({}),
    } as DOMRect);
    fireEvent.pointerDown(textLayer, {
      clientX: 78,
      clientY: 65,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerUp(textLayer, {
      clientX: 78,
      clientY: 65,
      pointerId: 1,
    });

    fireEvent.click(within(textLayer).getByRole("button", { name: "链接" }));
    fireEvent.change(screen.getByLabelText("链接网址"), {
      target: { value: "https://example.com" },
    });
    fireEvent.click(screen.getByRole("button", { name: "添加" }));

    await waitFor(() => expect(commandCalls("create_link")).toHaveLength(1));
    // The link mutation announces a document change, so the shared save state
    // refreshes availability from the unified history.
    await waitFor(() => expect(lastMenuState().get("edit.undo")).toBe(true));

    act(() => menuHandler?.({ payload: "edit.undo" }));
    await waitFor(() => expect(commandCalls("undo_text_edit")).toHaveLength(1));
  });

  it("keeps page and history shortcuts out of focused text controls", async () => {
    const undoable = {
      canUndo: true,
      canRedo: false,
      revision: 1,
      savedRevision: 0,
      dirty: true,
    };
    const undone = {
      canUndo: false,
      canRedo: true,
      revision: 0,
      savedRevision: 0,
      dirty: false,
    };
    let currentHistory = undoable;
    installDocumentBackend({
      edit_history: () => currentHistory,
      undo_text_edit: () => {
        currentHistory = undone;
        return {
          pageIndex: 0,
          pageCount: 3,
          pageStructureChanged: false,
          history: undone,
        };
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await waitFor(() => expect(lastMenuState().get("edit.undo")).toBe(true));

    const pageInput = screen.getByRole("spinbutton", { name: "当前页码" });
    act(() => pageInput.focus());

    // While the input has focus its native text undo and page scrolling stay
    // untouched: the items lose their accelerators and their commands.
    await waitFor(() => {
      const state = lastMenuState();
      expect(state.get("page.prev")).toBe(false);
      expect(state.get("page.next")).toBe(false);
      expect(state.get("edit.undo")).toBe(false);
      expect(state.get("edit.redo")).toBe(false);
      // Saving is not an editing key and stays available.
      expect(state.get("file.save")).toBe(true);
      expect(state.get("file.save_as")).toBe(true);
      expect(state.get("view.zoom_in")).toBe(true);
    });

    act(() => menuHandler?.({ payload: "page.next" }));
    act(() => menuHandler?.({ payload: "edit.undo" }));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(pageInput).toHaveValue(1);
    expect(commandCalls("undo_text_edit")).toHaveLength(0);

    // The page canvas does not own text editing, so shortcuts work again.
    act(() => {
      pageInput.blur();
      screen.getByLabelText("页面视口").focus();
    });
    await waitFor(() => {
      const state = lastMenuState();
      expect(state.get("page.next")).toBe(true);
      expect(state.get("edit.undo")).toBe(true);
    });

    act(() => menuHandler?.({ payload: "page.next" }));
    await waitFor(() => expect(pageInput).toHaveValue(2));
    act(() => menuHandler?.({ payload: "edit.undo" }));
    await waitFor(() => expect(commandCalls("undo_text_edit")).toHaveLength(1));
  });

  it("ignores page and history menu events that race ahead of the focus snapshot", async () => {
    installDocumentBackend({ edit_history: () => dirtyHistory });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await waitFor(() => expect(lastMenuState().get("page.next")).toBe(true));

    const pageInput = screen.getByRole("spinbutton", { name: "当前页码" });

    // The native menu is still enabled when focus lands in the input: the
    // disabled snapshot reaches Rust one command round trip later. A menu
    // event arriving in that window (an accelerator or a click) must yield to
    // the focused text control instead of navigating or undoing the document.
    act(() => {
      pageInput.focus();
      menuHandler?.({ payload: "page.next" });
      menuHandler?.({ payload: "edit.undo" });
    });
    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(pageInput).toHaveValue(1);
    expect(commandCalls("undo_text_edit")).toHaveLength(0);
  });

  it("accepts Ctrl+Y for redo on Windows without a second menu accelerator", async () => {
    const userAgent = vi
      .spyOn(window.navigator, "userAgent", "get")
      .mockReturnValue("Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
    try {
      const redoable = {
        canUndo: true,
        canRedo: true,
        revision: 2,
        savedRevision: 1,
        dirty: true,
      };
      const redone = {
        canUndo: true,
        canRedo: false,
        revision: 2,
        savedRevision: 2,
        dirty: false,
      };
      let currentHistory = redoable;
      installDocumentBackend({
        edit_history: () => currentHistory,
        redo_text_edit: () => {
          currentHistory = redone;
          return {
            pageIndex: 0,
            pageCount: 3,
            pageStructureChanged: false,
            history: redone,
          };
        },
      });
      openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
      const user = userEvent.setup();

      render(<App />);
      await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
      await screen.findByText("report.pdf");
      await waitFor(() => expect(lastMenuState().get("edit.redo")).toBe(true));

      const viewport = screen.getByLabelText("页面视口");
      const pageInput = screen.getByRole("spinbutton", { name: "当前页码" });

      // A focused text control keeps Ctrl+Y for its own native redo.
      act(() => pageInput.focus());
      fireEvent.keyDown(pageInput, { key: "y", ctrlKey: true });
      await new Promise((resolve) => setTimeout(resolve, 20));
      expect(commandCalls("redo_text_edit")).toHaveLength(0);

      act(() => {
        pageInput.blur();
        viewport.focus();
      });
      fireEvent.keyDown(viewport, { key: "y", ctrlKey: true });
      await waitFor(() =>
        expect(commandCalls("redo_text_edit")).toHaveLength(1),
      );

      // The fallback checks availability: once redo is exhausted it is inert.
      fireEvent.keyDown(viewport, { key: "y", ctrlKey: true });
      await new Promise((resolve) => setTimeout(resolve, 20));
      expect(commandCalls("redo_text_edit")).toHaveLength(1);
    } finally {
      userAgent.mockRestore();
    }
  });

  it("does not treat Ctrl+Y as redo outside Windows", async () => {
    installDocumentBackend({
      edit_history: () => ({
        canUndo: true,
        canRedo: true,
        revision: 2,
        savedRevision: 1,
        dirty: true,
      }),
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await waitFor(() => expect(lastMenuState().get("edit.redo")).toBe(true));

    // The platform fallback must not shadow the menu's own redo accelerator
    // outside Windows (where the test runner reports a non-Windows user agent).
    fireEvent.keyDown(screen.getByLabelText("页面视口"), {
      key: "y",
      ctrlKey: true,
    });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(commandCalls("redo_text_edit")).toHaveLength(0);
  });

  it("keeps page navigation but disables editing menu items for a restricted document", async () => {
    installDocumentBackend({
      open_document: () => ({ ...documentView, readOnly: true }),
      edit_history: () => dirtyHistory,
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await waitFor(() => {
      const state = lastMenuState();
      expect(state.get("file.save")).toBe(false);
      // Save As may still write a new copy of a restricted document.
      expect(state.get("file.save_as")).toBe(true);
      expect(state.get("edit.undo")).toBe(false);
      expect(state.get("edit.redo")).toBe(false);
      expect(state.get("page.prev")).toBe(false);
      expect(state.get("page.next")).toBe(true);
      expect(state.get("file.print")).toBe(true);
    });

    act(() => menuHandler?.({ payload: "page.next" }));
    await waitFor(() =>
      expect(screen.getByRole("spinbutton", { name: "当前页码" })).toHaveValue(
        2,
      ),
    );
    expect(commandCalls("undo_text_edit")).toHaveLength(0);
  });

  it("freezes every menu item while the close question is open", async () => {
    installDocumentBackend({ edit_history: () => dirtyHistory });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await triggerWindowClose();
    expect(closeConfirmDialog()).toBeInTheDocument();

    await waitFor(() => {
      const state = lastMenuState();
      for (const id of [
        "file.open",
        "file.print",
        "file.export_page",
        "file.save",
        "file.save_as",
        "app.settings",
        "edit.undo",
        "edit.redo",
        "view.actual_size",
        "view.zoom_in",
        "view.zoom_out",
        "view.fit_width",
        "view.fit_page",
        "view.fullscreen",
        "page.prev",
        "page.next",
      ]) {
        expect(state.get(id), id).toBe(false);
      }
    });
  });

  it("freezes the menu while the settings dialog is open", async () => {
    installDocumentBackend({ edit_history: () => dirtyHistory });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await waitFor(() => expect(lastMenuState().get("file.save")).toBe(true));

    await user.click(screen.getByRole("button", { name: "设置" }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    await waitFor(() => {
      const state = lastMenuState();
      expect(state.get("file.open")).toBe(false);
      expect(state.get("file.save")).toBe(false);
      expect(state.get("page.next")).toBe(false);
      expect(state.get("app.settings")).toBe(false);
    });
  });

  it("re-publishes the app-wide menu when the window regains focus", async () => {
    installDocumentBackend();
    render(<App />);
    await waitFor(() =>
      expect(commandCalls("set_menu_state").length).toBeGreaterThan(0),
    );
    const callsBefore = commandCalls("set_menu_state").length;
    const payloadBefore = commandCalls("set_menu_state").at(-1)?.[1];

    act(() => {
      window.dispatchEvent(new Event("focus"));
    });

    await waitFor(() =>
      expect(commandCalls("set_menu_state").length).toBeGreaterThan(
        callsBefore,
      ),
    );
    // The same window state is pushed again so a menu changed by another
    // document window cannot stay stale.
    expect(commandCalls("set_menu_state").at(-1)?.[1]).toEqual(payloadBefore);
  });

  it("reports a failed menu undo without losing the undo affordance", async () => {
    installDocumentBackend({
      edit_history: () => dirtyHistory,
      undo_text_edit: () => {
        throw new Error("history unavailable");
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await waitFor(() => expect(lastMenuState().get("edit.undo")).toBe(true));

    act(() => menuHandler?.({ payload: "edit.undo" }));

    expect(
      await screen.findByText("操作失败，请重试。"),
    ).toBeInTheDocument();
    expect(lastMenuState().get("edit.undo")).toBe(true);
  });

  it("clamps zoom to 10%-4000% and supports fit width, fit page and actual size", async () => {
    installDocumentBackend();
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toEqual({
        documentId: "doc-1",
        pageIndex: 0,
        scale: 1,
      });
    });

    const zoomInput = screen.getByRole("textbox", { name: "缩放比例" });
    expect(zoomInput).toHaveValue("100%");

    await user.clear(zoomInput);
    await user.type(zoomInput, "250%{Enter}");
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toEqual({
        documentId: "doc-1",
        pageIndex: 0,
        scale: 2.5,
      });
    });
    expect(zoomInput).toHaveValue("250%");
    expect(screen.getByText("第 1 / 3 页 · 250%")).toBeInTheDocument();

    // Zoom above 4000% is clamped, and the page is rendered as a viewport
    // region instead of an enormous full-page bitmap.
    setViewportSize(1000, 600);
    await user.clear(zoomInput);
    await user.type(zoomInput, "9000%{Enter}");
    await waitFor(() => expect(zoomInput).toHaveValue("4000%"));
    await waitFor(() => expect(renderRegionCalls().length).toBeGreaterThan(0));

    const region = renderRegionCalls().at(-1)!;
    expect(region).toMatchObject({
      documentId: "doc-1",
      pageIndex: 0,
      scale: 40,
    });
    // A 612x792 page at 4000% is 24480x31680 display pixels.
    expect(region.x).toBeGreaterThanOrEqual(0);
    expect(region.y).toBeGreaterThanOrEqual(0);
    expect(region.x + region.width).toBeLessThanOrEqual(24480);
    expect(region.y + region.height).toBeLessThanOrEqual(31680);
    expect(region.width).toBeGreaterThan(0);
    expect(region.width).toBeLessThanOrEqual(1400);
    expect(region.height).toBeGreaterThan(0);
    expect(region.height).toBeLessThanOrEqual(1000);
    expect(renderPageCalls().map((call) => call.scale)).not.toContain(40);

    // Zoom below 10% is clamped, and full-page rendering resumes.
    await user.clear(zoomInput);
    await user.type(zoomInput, "1%{Enter}");
    await waitFor(() => expect(zoomInput).toHaveValue("10%"));
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toEqual({
        documentId: "doc-1",
        pageIndex: 0,
        scale: 0.1,
      });
    });

    await user.click(screen.getByRole("button", { name: "放大" }));
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({ scale: 0.125 });
    });
    await user.click(screen.getByRole("button", { name: "缩小" }));
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({ scale: 0.1 });
    });

    // Fit width fills the measured viewport exactly: 1000 / 612 points.
    await user.click(screen.getByRole("button", { name: "适合宽度" }));
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toEqual({
        documentId: "doc-1",
        pageIndex: 0,
        scale: 1.634,
      });
    });
    expect(screen.getByRole("button", { name: "适合宽度" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("button", { name: "适合页面" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    expect(screen.getByText("第 1 / 3 页 · 163%")).toBeInTheDocument();

    // Fit page uses the limiting dimension: 600 / 792 points.
    await user.click(screen.getByRole("button", { name: "适合页面" }));
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toEqual({
        documentId: "doc-1",
        pageIndex: 0,
        scale: 0.7576,
      });
    });
    expect(screen.getByRole("button", { name: "适合页面" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByText("第 1 / 3 页 · 76%")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "实际大小" }));
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toEqual({
        documentId: "doc-1",
        pageIndex: 0,
        scale: 1,
      });
    });
    expect(screen.getByRole("button", { name: "适合页面" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    expect(screen.getByText("第 1 / 3 页 · 100%")).toBeInTheDocument();
  });

  it("lazily renders only visible thumbnails, caches them and jumps on click", async () => {
    const largeDocument = {
      ...documentView,
      name: "large.pdf",
      path: "/tmp/large.pdf",
      pageCount: 500,
    };
    installDocumentBackend({ open_document: () => largeDocument });
    openDialogMock.mockResolvedValueOnce("/tmp/large.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("large.pdf");
    await waitFor(() => expect(thumbnailCalls().length).toBeGreaterThan(0));

    const initial = thumbnailCalls();
    expect(initial.length).toBeLessThanOrEqual(10);
    expect(
      initial.every(
        (call) => call.documentId === "doc-1" && call.maxEdge <= 256,
      ),
    ).toBe(true);
    expect(new Set(initial.map((call) => call.pageIndex)).size).toBe(
      initial.length,
    );
    expect(initial.map((call) => call.pageIndex)).toContain(0);
    expect(initial.map((call) => call.pageIndex)).not.toContain(499);

    // Only the visible window is mounted; the other 494 pages are not in the DOM.
    expect(screen.queryByRole("button", { name: "第 500 页" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "第 1 页" })).toBeInTheDocument();
    expect(
      screen.getAllByRole("button", { name: /^第 \d+ 页$/ }).length,
    ).toBeLessThanOrEqual(8);

    // Scroll near page 300: thumbnails are requested for that window alone.
    const rail = screen.getByRole("navigation", { name: "页面缩略图" });
    Object.defineProperty(rail, "clientHeight", {
      configurable: true,
      value: 600,
    });
    Object.defineProperty(rail, "scrollTop", {
      configurable: true,
      writable: true,
      value: 300 * 168,
    });
    fireEvent.scroll(rail);

    await waitFor(() =>
      expect(thumbnailCalls().some((call) => call.pageIndex === 300)).toBe(true),
    );
    expect(thumbnailCalls().length).toBeLessThanOrEqual(initial.length + 10);
    expect(
      screen.getAllByRole("button", { name: /^第 \d+ 页$/ }).length,
    ).toBeLessThanOrEqual(8);
    expect(screen.getByRole("button", { name: "第 301 页" })).toBeInTheDocument();

    // Clicking a thumbnail jumps to that page.
    await user.click(screen.getByRole("button", { name: "第 301 页" }));
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({
        documentId: "doc-1",
        pageIndex: 300,
        scale: 1,
      });
    });
    expect(screen.getByText("第 301 / 500 页 · 100%")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "第 301 页" })).toHaveAttribute(
      "aria-current",
      "page",
    );

    // Scrolling away and back reuses cached thumbnails.
    const pageZeroCalls = thumbnailCalls().filter(
      (call) => call.pageIndex === 0,
    ).length;
    const pageThreeHundredCalls = thumbnailCalls().filter(
      (call) => call.pageIndex === 300,
    ).length;

    Object.defineProperty(rail, "scrollTop", {
      configurable: true,
      writable: true,
      value: 0,
    });
    fireEvent.scroll(rail);
    await act(async () => {
      await Promise.resolve();
    });

    expect(
      thumbnailCalls().filter((call) => call.pageIndex === 0).length,
    ).toBe(pageZeroCalls);
    expect(
      thumbnailCalls().filter((call) => call.pageIndex === 300).length,
    ).toBe(pageThreeHundredCalls);
  });

  it("reveals the active thumbnail when jumping with the page input", async () => {
    const largeDocument = {
      ...documentView,
      name: "large.pdf",
      path: "/tmp/large.pdf",
      pageCount: 500,
    };
    installDocumentBackend({ open_document: () => largeDocument });
    openDialogMock.mockResolvedValueOnce("/tmp/large.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("large.pdf");
    await waitFor(() => expect(thumbnailCalls().length).toBeGreaterThan(0));

    const rail = screen.getByRole("navigation", { name: "页面缩略图" });
    Object.defineProperty(rail, "clientHeight", {
      configurable: true,
      value: 600,
    });
    let railScrollTop = 0;
    Object.defineProperty(rail, "scrollTop", {
      configurable: true,
      get: () => railScrollTop,
      set: (value: number) => {
        railScrollTop = value;
      },
    });

    const pageInput = screen.getByLabelText("当前页码");
    fireEvent.change(pageInput, { target: { value: "400" } });
    fireEvent.keyDown(pageInput, { key: "Enter" });

    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({
        documentId: "doc-1",
        pageIndex: 399,
      });
    });

    // The rail scrolls the active page into view and lazily loads its window
    // instead of every thumbnail between pages 1 and 400.
    expect(railScrollTop).toBe(400 * 168 - 600);
    await waitFor(() =>
      expect(
        thumbnailCalls().some(
          (call) => Math.abs(call.pageIndex - 399) <= 2,
        ),
      ).toBe(true),
    );
    expect(thumbnailCalls().length).toBeLessThanOrEqual(20);
  });

  it("restores the previous page and zoom after leaving fullscreen", async () => {
    installDocumentBackend();
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({
        pageIndex: 0,
        scale: 1,
      });
    });

    // Pick a page and zoom that must come back after fullscreen.
    await user.click(screen.getByRole("button", { name: "下一页" }));
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({ pageIndex: 1 });
    });
    const zoomInput = screen.getByRole("textbox", { name: "缩放比例" });
    await user.clear(zoomInput);
    await user.type(zoomInput, "200%{Enter}");
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({
        pageIndex: 1,
        scale: 2,
      });
    });

    await user.click(screen.getByRole("button", { name: "全屏" }));
    expect(setFullscreenMock).toHaveBeenCalledWith(true);
    expect(
      await screen.findByRole("button", { name: "退出全屏" }),
    ).toBeInTheDocument();

    // While fullscreen, navigate and zoom somewhere else.
    await user.click(screen.getByRole("button", { name: "下一页" }));
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({ pageIndex: 2 });
    });
    await user.clear(zoomInput);
    await user.type(zoomInput, "50%{Enter}");
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({
        pageIndex: 2,
        scale: 0.5,
      });
    });

    await user.click(screen.getByRole("button", { name: "退出全屏" }));
    expect(setFullscreenMock).toHaveBeenLastCalledWith(false);
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toEqual({
        documentId: "doc-1",
        pageIndex: 1,
        scale: 2,
      });
    });
    expect(screen.getByText("第 2 / 3 页 · 200%")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "全屏" })).toBeInTheDocument();

    // Escape leaves fullscreen through the same path.
    await user.click(screen.getByRole("button", { name: "全屏" }));
    expect(
      await screen.findByRole("button", { name: "退出全屏" }),
    ).toBeInTheDocument();
    act(() => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    await waitFor(() => {
      expect(setFullscreenMock).toHaveBeenLastCalledWith(false);
    });
    expect(screen.getByRole("button", { name: "全屏" })).toBeInTheDocument();
  });

  it("lays out rotated and mixed-size pages without clipping or aspect errors", async () => {
    const rotatedInfo = (pageIndex: number) => {
      switch (pageIndex) {
        case 0:
          return {
            pageIndex,
            width: 792,
            height: 612,
            rotation: 90,
            cropX: 0,
            cropY: 0,
          };
        case 1:
          return {
            pageIndex,
            width: 432,
            height: 288,
            rotation: 270,
            cropX: 0,
            cropY: 0,
          };
        default:
          return {
            pageIndex,
            width: 288,
            height: 432,
            rotation: 180,
            cropX: 0,
            cropY: 0,
          };
      }
    };
    installDocumentBackend({
      page_info: (args) =>
        rotatedInfo((args as { pageIndex: number }).pageIndex),
      render_page: (args) => {
        const { pageIndex, scale } = args as {
          pageIndex: number;
          scale: number;
        };
        const info = rotatedInfo(pageIndex);
        return {
          documentId: "doc-1",
          pageIndex,
          scale,
          width: Math.round(info.width * scale),
          height: Math.round(info.height * scale),
          pngBase64: "iVBORw0KGgo=",
        };
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/rotated.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    // Page 1 is portrait in PDF user space but displays landscape (90 degrees).
    const first = await screen.findByRole("img", { name: "第 1 页" });
    await waitFor(() => {
      expect(first).toHaveStyle({ width: "792px", height: "612px" });
    });
    expect(first).toHaveAttribute("data-page-rotation", "90");
    expect(first).toHaveAttribute("data-page-width", "792");
    expect(first).toHaveAttribute("data-page-height", "612");

    await user.click(screen.getByRole("button", { name: "下一页" }));
    const second = await screen.findByRole("img", { name: "第 2 页" });
    await waitFor(() => {
      expect(second).toHaveStyle({ width: "432px", height: "288px" });
    });
    expect(second).toHaveAttribute("data-page-rotation", "270");
    expect(second).toHaveAttribute("data-page-width", "432");
    expect(second).toHaveAttribute("data-page-height", "288");

    // Zooming preserves the rotated display aspect for the current page.
    const zoomInput = screen.getByRole("textbox", { name: "缩放比例" });
    await user.clear(zoomInput);
    await user.type(zoomInput, "200%{Enter}");
    await waitFor(() => {
      expect(second).toHaveStyle({ width: "864px", height: "576px" });
    });
    expect(renderPageCalls().at(-1)).toMatchObject({
      pageIndex: 1,
      scale: 2,
    });

    // The next page has a different media size and 180 degree rotation.
    await user.click(screen.getByRole("button", { name: "下一页" }));
    const third = await screen.findByRole("img", { name: "第 3 页" });
    await waitFor(() => {
      expect(third).toHaveStyle({ width: "576px", height: "864px" });
    });
    expect(third).toHaveAttribute("data-page-rotation", "180");
    expect(third).toHaveAttribute("data-page-width", "288");
    expect(third).toHaveAttribute("data-page-height", "432");
  });

  it("wires native menu ids to open and zoom actions", async () => {
    installDocumentBackend();
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await screen.findByRole("region", { name: "文本层" });
    await waitFor(() => expect(menuHandler).toBeDefined());

    act(() => menuHandler?.({ payload: "view.zoom_in" }));
    await waitFor(() => {
      expect(renderPageCalls()).toContainEqual({
        documentId: "doc-1",
        pageIndex: 0,
        scale: 1.25,
      });
    });

    act(() => menuHandler?.({ payload: "view.actual_size" }));
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toEqual({
        documentId: "doc-1",
        pageIndex: 0,
        scale: 1,
      });
    });

    // The layer is page-space geometry overlaid on the canvas; zooming
    // re-renders the bitmap but must not re-fetch or re-parse the text.
    expect(commandCalls("extract_text_layer")).toHaveLength(1);

    openDialogMock.mockResolvedValueOnce(null);
    act(() => menuHandler?.({ payload: "file.open" }));
    await waitFor(() => expect(openDialogMock).toHaveBeenCalledTimes(2));
  });

  it("wires native menu ids to fit and fullscreen actions", async () => {
    installDocumentBackend();
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await waitFor(() => expect(menuHandler).toBeDefined());

    setViewportSize(1000, 600);
    act(() => menuHandler?.({ payload: "view.fit_width" }));
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({ scale: 1.634 });
    });
    expect(screen.getByRole("button", { name: "适合宽度" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    act(() => menuHandler?.({ payload: "view.fit_page" }));
    await waitFor(() => {
      expect(renderPageCalls().at(-1)).toMatchObject({ scale: 0.7576 });
    });

    act(() => menuHandler?.({ payload: "view.fullscreen" }));
    await waitFor(() => expect(setFullscreenMock).toHaveBeenCalledWith(true));
    expect(
      await screen.findByRole("button", { name: "退出全屏" }),
    ).toBeInTheDocument();

    act(() => menuHandler?.({ payload: "view.fullscreen" }));
    await waitFor(() =>
      expect(setFullscreenMock).toHaveBeenLastCalledWith(false),
    );
    expect(screen.getByRole("button", { name: "全屏" })).toBeInTheDocument();
  });

  it("auto-opens the document named in the window query string", async () => {
    installDocumentBackend();
    window.history.replaceState({}, "", "/?open=%2Ftmp%2Freport.pdf");

    render(<App />);

    await waitFor(() => {
      expect(commandCalls("open_document")).toEqual([
        ["open_document", { path: "/tmp/report.pdf", password: null }],
      ]);
    });
    expect(await screen.findByText("report.pdf")).toBeInTheDocument();
  });

  it("closes a clean document without a confirmation dialog", async () => {
    installDocumentBackend();
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    const preventDefault = await triggerWindowClose();

    expect(preventDefault).toHaveBeenCalled();
    expect(
      screen.queryByRole("dialog", { name: "关闭前保存更改？" }),
    ).not.toBeInTheDocument();
    expect(commandCalls("close_document")).toEqual([
      ["close_document", { documentId: "doc-1" }],
    ]);
    expect(destroyWindowMock).toHaveBeenCalledTimes(1);
  });

  it("shows the unsaved marker from backend history and clears it on undo to the saved revision", async () => {
    let history = idleHistory;
    installDocumentBackend({ edit_history: () => history });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    expect(screen.queryByText("未保存")).not.toBeInTheDocument();
    expect(setTitle).toHaveBeenLastCalledWith("report.pdf — PDFX");

    // The edit commits in the backend; its page-change event asks the save
    // hook to re-read the command-boundary history.
    history = dirtyHistory;
    act(() => {
      notifyDocumentChanged({ documentId: "doc-1", pageIndex: 0 });
    });
    expect(await screen.findByText("未保存")).toBeInTheDocument();
    await waitFor(() =>
      expect(setTitle).toHaveBeenLastCalledWith("report.pdf（未保存） — PDFX"),
    );

    // Undoing back to the saved revision is clean again without another save.
    history = cleanHistory;
    act(() => {
      notifyDocumentChanged({ documentId: "doc-1", pageIndex: 0 });
    });
    await waitFor(() =>
      expect(screen.queryByText("未保存")).not.toBeInTheDocument(),
    );
    expect(setTitle).toHaveBeenLastCalledWith("report.pdf — PDFX");

    // Redo past the saved revision restores the marker.
    history = { ...dirtyHistory, revision: 3, savedRevision: 2 };
    act(() => {
      notifyDocumentChanged({ documentId: "doc-1", pageIndex: 0 });
    });
    expect(await screen.findByText("未保存")).toBeInTheDocument();
  });

  it("clears the unsaved marker after a successful save and restores it after another edit", async () => {
    let history = dirtyHistory;
    installDocumentBackend({
      edit_history: () => history,
      save_document: () => {
        history = { ...cleanHistory, revision: 1 };
        return savedResult({ history });
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    saveDialogMock.mockResolvedValueOnce("/tmp/copy.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    expect(await screen.findByText("未保存")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "另存为…" }));
    await screen.findByText(/已保存到/);
    await waitFor(() =>
      expect(screen.queryByText("未保存")).not.toBeInTheDocument(),
    );
    expect(setTitle).toHaveBeenLastCalledWith("report.pdf — PDFX");

    history = { ...dirtyHistory, revision: 2, savedRevision: 1 };
    act(() => {
      notifyDocumentChanged({ documentId: "doc-1", pageIndex: 0 });
    });
    expect(await screen.findByText("未保存")).toBeInTheDocument();
    await waitFor(() =>
      expect(setTitle).toHaveBeenLastCalledWith("report.pdf（未保存） — PDFX"),
    );
  });

  it("asks before closing an unsaved document and keeps the window open on cancel", async () => {
    installDocumentBackend({ edit_history: () => dirtyHistory });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await triggerWindowClose();

    const dialog = closeConfirmDialog();
    expect(
      within(dialog).getByRole("button", { name: "保存" }),
    ).toBeEnabled();
    expect(
      within(dialog).getByRole("button", { name: "不保存" }),
    ).toBeEnabled();
    const cancel = within(dialog).getByRole("button", { name: "取消" });
    expect(cancel).toHaveFocus();

    await user.click(cancel);

    expect(
      screen.queryByRole("dialog", { name: "关闭前保存更改？" }),
    ).not.toBeInTheDocument();
    expect(commandCalls("save_document")).toHaveLength(0);
    expect(commandCalls("close_document")).toHaveLength(0);
    expect(destroyWindowMock).not.toHaveBeenCalled();
  });

  it("ignores save accelerators while the close question is open", async () => {
    installDocumentBackend({ edit_history: () => dirtyHistory });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await triggerWindowClose();
    await waitFor(() => expect(menuHandler).toBeDefined());
    act(() => menuHandler?.({ payload: "file.save" }));
    act(() => menuHandler?.({ payload: "file.save_as" }));

    expect(commandCalls("save_document")).toHaveLength(0);
    expect(closeConfirmDialog()).toBeInTheDocument();

    await user.click(
      within(closeConfirmDialog()).getByRole("button", { name: "取消" }),
    );
    expect(
      screen.queryByRole("dialog", { name: "关闭前保存更改？" }),
    ).not.toBeInTheDocument();
    expect(destroyWindowMock).not.toHaveBeenCalled();
  });

  it("closes without saving when the user chooses not to save", async () => {
    installDocumentBackend({ edit_history: () => dirtyHistory });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await triggerWindowClose();
    await user.click(
      within(closeConfirmDialog()).getByRole("button", { name: "不保存" }),
    );

    await waitFor(() => expect(destroyWindowMock).toHaveBeenCalledTimes(1));
    expect(commandCalls("save_document")).toHaveLength(0);
    expect(commandCalls("close_document")).toEqual([
      ["close_document", { documentId: "doc-1" }],
    ]);
  });

  it("saves through the normal Save As flow when closing and closes after the write", async () => {
    let history = dirtyHistory;
    installDocumentBackend({
      edit_history: () => history,
      save_document: () => {
        history = { ...cleanHistory, revision: 1 };
        return savedResult({ history });
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    saveDialogMock.mockResolvedValueOnce("/tmp/copy.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await triggerWindowClose();
    await user.click(
      within(closeConfirmDialog()).getByRole("button", { name: "保存" }),
    );

    await waitFor(() => {
      expect(commandCalls("save_document")).toEqual([
        [
          "save_document",
          {
            documentId: "doc-1",
            request: {
              saveAsPath: "/tmp/copy.pdf",
              confirmUnencryptedOutput: false,
            },
          },
        ],
      ]);
    });
    await waitFor(() => expect(destroyWindowMock).toHaveBeenCalledTimes(1));
    expect(commandCalls("close_document")).toEqual([
      ["close_document", { documentId: "doc-1" }],
    ]);
    expect(
      screen.queryByRole("dialog", { name: "关闭前保存更改？" }),
    ).not.toBeInTheDocument();
  });

  it("saves changes over the existing output when closing after a first save", async () => {
    let history = idleHistory;
    installDocumentBackend({
      edit_history: () => history,
      save_document: (args) => {
        history = { ...cleanHistory, revision: 2 };
        return savedResult({
          kind: (args as { request: { saveAsPath: string | null } }).request
            .saveAsPath
            ? "save_as"
            : "save_changes",
          history,
        });
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    saveDialogMock.mockResolvedValueOnce("/tmp/copy.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    // First save establishes the Output Document, exactly as the toolbar does.
    await user.click(screen.getByRole("button", { name: "另存为…" }));
    await screen.findByText(/已保存到/);

    history = { ...dirtyHistory, revision: 3, savedRevision: 2 };
    act(() => {
      notifyDocumentChanged({ documentId: "doc-1", pageIndex: 0 });
    });
    expect(await screen.findByText("未保存")).toBeInTheDocument();

    await triggerWindowClose();
    await user.click(
      within(closeConfirmDialog()).getByRole("button", { name: "保存" }),
    );

    await waitFor(() => {
      const requests = commandCalls("save_document").map(
        ([, args]) =>
          (args as { request: { saveAsPath: string | null } }).request,
      );
      expect(requests).toEqual([
        { saveAsPath: "/tmp/copy.pdf", confirmUnencryptedOutput: false },
        { saveAsPath: null, confirmUnencryptedOutput: false },
      ]);
    });
    await waitFor(() => expect(destroyWindowMock).toHaveBeenCalledTimes(1));
    expect(saveDialogMock).toHaveBeenCalledTimes(1);
  });

  it("keeps the window open when the close-triggered save fails", async () => {
    installDocumentBackend({
      edit_history: () => dirtyHistory,
      save_document: () => {
        throw { code: "save_failed", message: "磁盘空间不足" };
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    saveDialogMock.mockResolvedValueOnce("/tmp/copy.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await triggerWindowClose();
    await user.click(
      within(closeConfirmDialog()).getByRole("button", { name: "保存" }),
    );

    expect(
      await screen.findByText("保存失败，窗口未关闭。请取消后重试保存。"),
    ).toBeInTheDocument();
    expect(
      within(closeConfirmDialog()).getByRole("button", { name: "保存" }),
    ).toBeEnabled();
    expect(destroyWindowMock).not.toHaveBeenCalled();
    expect(commandCalls("close_document")).toHaveLength(0);

    // The normal retryable save error is available once the question closes.
    await user.click(
      within(closeConfirmDialog()).getByRole("button", { name: "取消" }),
    );
    expect(screen.getByText("重试保存")).toBeInTheDocument();
  });

  it("keeps the window open when the Save As dialog is cancelled during close", async () => {
    installDocumentBackend({ edit_history: () => dirtyHistory });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    saveDialogMock.mockResolvedValueOnce(null);
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await triggerWindowClose();
    await user.click(
      within(closeConfirmDialog()).getByRole("button", { name: "保存" }),
    );

    await waitFor(() => expect(saveDialogMock).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(
        within(closeConfirmDialog()).getByRole("button", { name: "保存" }),
      ).toBeEnabled(),
    );
    expect(commandCalls("save_document")).toHaveLength(0);
    expect(commandCalls("close_document")).toHaveLength(0);
    expect(destroyWindowMock).not.toHaveBeenCalled();
  });

  it("keeps the window open when required fields block the close-triggered save", async () => {
    const requiredField = {
      fieldId: "1:1",
      name: "notes",
      kind: "text",
      pageIndex: 0,
      order: 0,
      rect: { x: 72, y: 72, width: 300, height: 60 },
      value: null,
      checked: false,
      options: [],
      selectedIndex: null,
      required: true,
      readOnly: false,
      multiline: true,
      radioGroup: null,
      exportValue: null,
    };
    installDocumentBackend({
      edit_history: () => dirtyHistory,
      form_fields: () => [requiredField],
      missing_required_fields: () => [requiredField],
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await triggerWindowClose();
    await user.click(
      within(closeConfirmDialog()).getByRole("button", { name: "保存" }),
    );

    expect(
      await screen.findByText(
        "有必填字段尚未填写，窗口未关闭。请取消后补全再试。",
      ),
    ).toBeInTheDocument();
    expect(saveDialogMock).not.toHaveBeenCalled();
    expect(commandCalls("save_document")).toHaveLength(0);
    expect(destroyWindowMock).not.toHaveBeenCalled();
  });

  it("confirms the unencrypted output before closing an encrypted document", async () => {
    let history = dirtyHistory;
    installDocumentBackend({
      open_document: () => ({ ...documentView, encrypted: true }),
      edit_history: () => history,
      save_document: (args) => {
        expect(
          (args as { request: { confirmUnencryptedOutput: boolean } }).request
            .confirmUnencryptedOutput,
        ).toBe(true);
        history = { ...cleanHistory, revision: 1 };
        return savedResult({ history });
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/encrypted.pdf");
    saveDialogMock.mockResolvedValueOnce("/tmp/copy.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await triggerWindowClose();
    await user.click(
      within(closeConfirmDialog()).getByRole("button", { name: "保存" }),
    );

    const confirmation = await screen.findByRole("dialog", {
      name: "输出将不加密",
    });
    // The close question waits behind the confirmation and cannot be answered.
    expect(
      within(closeConfirmDialog()).getByRole("button", { name: "取消" }),
    ).toBeDisabled();

    await user.click(
      within(confirmation).getByRole("button", {
        name: "继续保存（不加密）",
      }),
    );

    await waitFor(() => expect(destroyWindowMock).toHaveBeenCalledTimes(1));
    expect(commandCalls("save_document")).toHaveLength(1);
    expect(commandCalls("close_document")).toHaveLength(1);
  });

  it("reads the dirty state from the backend on close even before its change event arrives", async () => {
    let history = idleHistory;
    installDocumentBackend({ edit_history: () => history });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    expect(screen.queryByText("未保存")).not.toBeInTheDocument();

    // The edit committed in the backend, but its page-change event has not been
    // delivered yet. The close guard must not miss it.
    history = dirtyHistory;
    await triggerWindowClose();

    expect(closeConfirmDialog()).toBeInTheDocument();
    expect(commandCalls("close_document")).toHaveLength(0);
    expect(destroyWindowMock).not.toHaveBeenCalled();
  });

  it("ignores repeated close requests while the confirmation is open", async () => {
    installDocumentBackend({ edit_history: () => dirtyHistory });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await triggerWindowClose();
    const preventDefault = vi.fn();
    await act(async () => {
      await windowCloseHandler()({ preventDefault });
    });

    expect(preventDefault).toHaveBeenCalled();
    expect(screen.getAllByRole("dialog", { name: "关闭前保存更改？" })).toHaveLength(
      1,
    );
    expect(commandCalls("close_document")).toHaveLength(0);
    expect(destroyWindowMock).not.toHaveBeenCalled();
  });

  it("cancels the close confirmation with Escape", async () => {
    installDocumentBackend({ edit_history: () => dirtyHistory });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await triggerWindowClose();
    await user.keyboard("{Escape}");

    expect(
      screen.queryByRole("dialog", { name: "关闭前保存更改？" }),
    ).not.toBeInTheDocument();
    expect(commandCalls("close_document")).toHaveLength(0);
    expect(destroyWindowMock).not.toHaveBeenCalled();
  });

  it("waits for an in-flight save before closing a document it cleaned", async () => {
    let history = dirtyHistory;
    let resolveSave: ((value: unknown) => void) | undefined;
    const pendingSave = new Promise((resolve) => {
      resolveSave = resolve;
    });
    installDocumentBackend({
      edit_history: () => history,
      save_document: () => pendingSave,
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    saveDialogMock.mockResolvedValueOnce("/tmp/copy.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await user.click(screen.getByRole("button", { name: "另存为…" }));
    await waitFor(() =>
      expect(commandCalls("save_document")).toHaveLength(1),
    );

    const closePromise = windowCloseHandler()({ preventDefault: vi.fn() });
    history = { ...cleanHistory, revision: 1 };
    await act(async () => {
      resolveSave?.(savedResult({ history }));
      await closePromise;
    });

    expect(
      screen.queryByRole("dialog", { name: "关闭前保存更改？" }),
    ).not.toBeInTheDocument();
    expect(commandCalls("close_document")).toEqual([
      ["close_document", { documentId: "doc-1" }],
    ]);
    expect(destroyWindowMock).toHaveBeenCalledTimes(1);
  });

  it("saves a new document through the Save As dialog", async () => {
    installDocumentBackend({ save_document: () => savedResult() });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    saveDialogMock.mockResolvedValueOnce("/tmp/copy.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    expect(setTitle).toHaveBeenLastCalledWith("report.pdf — PDFX");

    await user.click(screen.getByRole("button", { name: "另存为…" }));

    await waitFor(() => {
      expect(commandCalls("save_document")).toEqual([
        [
          "save_document",
          {
            documentId: "doc-1",
            request: {
              saveAsPath: "/tmp/copy.pdf",
              confirmUnencryptedOutput: false,
            },
          },
        ],
      ]);
    });
    expect(saveDialogMock).toHaveBeenCalledWith(
      expect.objectContaining({
        defaultPath: "report.pdf",
        filters: [{ name: "PDF", extensions: ["pdf"] }],
      }),
    );
    expect(await screen.findByText(/已保存到/)).toHaveTextContent("copy.pdf");
  });

  it("does not save when the Save As dialog is cancelled", async () => {
    installDocumentBackend();
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    saveDialogMock.mockResolvedValueOnce(null);
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await user.click(screen.getByRole("button", { name: "另存为…" }));

    expect(saveDialogMock).toHaveBeenCalledTimes(1);
    expect(commandCalls("save_document")).toHaveLength(0);
  });

  it("overwrites the chosen Output Document on Save Changes without another dialog", async () => {
    installDocumentBackend({
      save_document: (args) =>
        savedResult({
          kind: (args as { request: { saveAsPath: string | null } }).request
            .saveAsPath
            ? "save_as"
            : "save_changes",
          backupPath: "/tmp/copy.pdfx-backup.pdf",
        }),
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    saveDialogMock.mockResolvedValueOnce("/tmp/copy.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await user.click(screen.getByRole("button", { name: "另存为…" }));
    await screen.findByText(/已保存到/);

    await user.click(screen.getByRole("button", { name: "保存更改" }));

    await waitFor(() => {
      expect(commandCalls("save_document")).toEqual([
        [
          "save_document",
          {
            documentId: "doc-1",
            request: {
              saveAsPath: "/tmp/copy.pdf",
              confirmUnencryptedOutput: false,
            },
          },
        ],
        [
          "save_document",
          {
            documentId: "doc-1",
            request: {
              saveAsPath: null,
              confirmUnencryptedOutput: false,
            },
          },
        ],
      ]);
    });
    expect(saveDialogMock).toHaveBeenCalledTimes(1);
  });

  it("routes the first Save Changes through Save As because no output exists yet", async () => {
    installDocumentBackend({ save_document: () => savedResult() });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    saveDialogMock.mockResolvedValueOnce("/tmp/first.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await user.click(screen.getByRole("button", { name: "保存更改" }));

    expect(saveDialogMock).toHaveBeenCalledTimes(1);
    await waitFor(() => {
      expect(commandCalls("save_document")).toEqual([
        [
          "save_document",
          {
            documentId: "doc-1",
            request: {
              saveAsPath: "/tmp/first.pdf",
              confirmUnencryptedOutput: false,
            },
          },
        ],
      ]);
    });
  });

  it("requires confirmation before writing an unencrypted copy of an encrypted document", async () => {
    installDocumentBackend({
      open_document: () => ({ ...documentView, name: "secret.pdf", encrypted: true }),
      save_document: () =>
        savedResult({ outputPath: "/tmp/secret-copy.pdf" }),
    });
    openDialogMock.mockResolvedValueOnce("/tmp/secret.pdf");
    saveDialogMock.mockResolvedValue("/tmp/secret-copy.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("secret.pdf");
    await user.click(screen.getByRole("button", { name: "另存为…" }));

    const dialog = await screen.findByRole("dialog", { name: "输出将不加密" });
    expect(commandCalls("save_document")).toHaveLength(0);

    await user.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(
      screen.queryByRole("dialog", { name: "输出将不加密" }),
    ).not.toBeInTheDocument();
    expect(commandCalls("save_document")).toHaveLength(0);

    await user.click(screen.getByRole("button", { name: "另存为…" }));
    await user.click(
      await screen.findByRole("button", { name: "继续保存（不加密）" }),
    );
    await waitFor(() => {
      expect(commandCalls("save_document")).toEqual([
        [
          "save_document",
          {
            documentId: "doc-1",
            request: {
              saveAsPath: "/tmp/secret-copy.pdf",
              confirmUnencryptedOutput: true,
            },
          },
        ],
      ]);
    });

    // Save Changes asks for the same confirmation on every write.
    await screen.findByText(/已保存到/);
    await user.click(screen.getByRole("button", { name: "保存更改" }));
    await user.click(
      await screen.findByRole("button", { name: "继续保存（不加密）" }),
    );
    await waitFor(() => {
      expect(commandCalls("save_document")).toHaveLength(2);
    });
    expect(commandCalls("save_document")[1]).toEqual([
      "save_document",
      {
        documentId: "doc-1",
        request: {
          saveAsPath: null,
          confirmUnencryptedOutput: true,
        },
      },
    ]);
  });

  it("lets a read-only document write an unencrypted copy after confirmation", async () => {
    installDocumentBackend({
      open_document: () => ({
        ...documentView,
        name: "restricted.pdf",
        encrypted: true,
        readOnly: true,
      }),
      save_document: () =>
        savedResult({ outputPath: "/tmp/restricted-copy.pdf" }),
    });
    openDialogMock.mockResolvedValueOnce("/tmp/restricted.pdf");
    saveDialogMock.mockResolvedValueOnce("/tmp/restricted-copy.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("restricted.pdf");

    expect(screen.getByRole("button", { name: "保存更改" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "另存为…" }));
    await user.click(
      await screen.findByRole("button", { name: "继续保存（不加密）" }),
    );

    await waitFor(() => {
      expect(commandCalls("save_document")).toEqual([
        [
          "save_document",
          {
            documentId: "doc-1",
            request: {
              saveAsPath: "/tmp/restricted-copy.pdf",
              confirmUnencryptedOutput: true,
            },
          },
        ],
      ]);
    });
  });

  it("shows a retryable error when a save fails and retries the same request", async () => {
    let attempts = 0;
    installDocumentBackend({
      save_document: async () => {
        attempts += 1;
        if (attempts === 1) {
          throw { code: "save_failed", message: "disk full while writing" };
        }
        return savedResult();
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    saveDialogMock.mockResolvedValueOnce("/tmp/copy.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await user.click(screen.getByRole("button", { name: "另存为…" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("保存失败");
    expect(alert).toHaveTextContent("disk full while writing");

    await user.click(screen.getByRole("button", { name: "重试保存" }));

    await waitFor(() => expect(attempts).toBe(2));
    expect(await screen.findByText(/已保存到/)).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("chooses a new path when the Save As target is the source document", async () => {
    let attempts = 0;
    installDocumentBackend({
      save_document: async (args) => {
        attempts += 1;
        const request = (args as { request: { saveAsPath: string | null } }).request;
        if (attempts === 1) {
          throw { code: "save_target_is_source", message: "choose another path" };
        }
        return savedResult({ outputPath: request.saveAsPath });
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    saveDialogMock
      .mockResolvedValueOnce("/tmp/report.pdf")
      .mockResolvedValueOnce("/tmp/report-copy.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await user.click(screen.getByRole("button", { name: "另存为…" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "不能把源文件本身作为输出",
    );
    await user.click(screen.getByRole("button", { name: "重新选择…" }));

    await waitFor(() => expect(attempts).toBe(2));
    expect(saveDialogMock).toHaveBeenCalledTimes(2);
    expect(commandCalls("save_document")[1]).toEqual([
      "save_document",
      {
        documentId: "doc-1",
        request: {
          saveAsPath: "/tmp/report-copy.pdf",
          confirmUnencryptedOutput: false,
        },
      },
    ]);
  });

  it("reports save progress while the write is in flight", async () => {
    let resolveSave: (value: unknown) => void = () => {};
    installDocumentBackend({
      save_document: () =>
        new Promise((resolve) => {
          resolveSave = resolve;
        }),
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    saveDialogMock.mockResolvedValueOnce("/tmp/copy.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await waitFor(() => expect(saveProgressHandler).toBeDefined());
    await user.click(screen.getByRole("button", { name: "另存为…" }));
    await waitFor(() => expect(commandCalls("save_document")).toHaveLength(1));

    act(() => {
      saveProgressHandler?.({
        payload: {
          documentId: "doc-1",
          stage: "serializing",
          completed: 1,
          total: 4,
        },
      });
    });
    expect(await screen.findByText("正在生成 PDF…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "另存为…" })).toBeDisabled();

    act(() => {
      saveProgressHandler?.({
        payload: {
          documentId: "doc-1",
          stage: "writing",
          completed: 3,
          total: 4,
        },
      });
    });
    expect(await screen.findByText("正在写入文件…")).toBeInTheDocument();

    await act(async () => {
      resolveSave(savedResult());
    });
    expect(await screen.findByText(/已保存到/)).toBeInTheDocument();
    expect(screen.queryByText("正在写入文件…")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "保存更改" })).toBeEnabled();
  });

  it("wires the file.save and file.save_as menu ids to the save flow", async () => {
    installDocumentBackend({ save_document: () => savedResult() });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    saveDialogMock.mockResolvedValueOnce("/tmp/copy.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await waitFor(() => expect(menuHandler).toBeDefined());

    act(() => menuHandler?.({ payload: "file.save_as" }));
    await screen.findByText(/已保存到/);
    expect(saveDialogMock).toHaveBeenCalledTimes(1);

    act(() => menuHandler?.({ payload: "file.save" }));
    await waitFor(() => expect(commandCalls("save_document")).toHaveLength(2));
    expect(saveDialogMock).toHaveBeenCalledTimes(1);
    expect(commandCalls("save_document")[1]).toEqual([
      "save_document",
      {
        documentId: "doc-1",
        request: {
          saveAsPath: null,
          confirmUnencryptedOutput: false,
        },
      },
    ]);
  });

  it("opens settings from the header button and the app menu", async () => {
    installBackend();
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "设置" }));
    expect(await screen.findByRole("dialog", { name: "设置" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "关闭设置" }));
    expect(screen.queryByRole("dialog", { name: "设置" })).not.toBeInTheDocument();

    await waitFor(() => expect(menuHandler).toBeDefined());
    act(() => menuHandler?.({ payload: "app.settings" }));
    expect(await screen.findByRole("dialog", { name: "设置" })).toBeInTheDocument();
  });

  it("opens print and export from the toolbar and the native menu", async () => {
    installDocumentBackend();
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await user.click(screen.getByRole("button", { name: "打印…" }));
    const printDialog = await screen.findByRole("dialog", { name: "打印" });
    await user.click(
      within(printDialog).getByRole("button", { name: "取消" }),
    );
    expect(screen.queryByRole("dialog", { name: "打印" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "导出当前页…" }));
    const exportDialog = await screen.findByRole("dialog", { name: "导出当前页" });
    await user.click(
      within(exportDialog).getByRole("button", { name: "取消" }),
    );
    expect(
      screen.queryByRole("dialog", { name: "导出当前页" }),
    ).not.toBeInTheDocument();

    await waitFor(() => expect(menuHandler).toBeDefined());
    act(() => menuHandler?.({ payload: "file.print" }));
    const menuPrintDialog = await screen.findByRole("dialog", { name: "打印" });
    await user.click(
      within(menuPrintDialog).getByRole("button", { name: "取消" }),
    );

    act(() => menuHandler?.({ payload: "file.export_page" }));
    expect(
      await screen.findByRole("dialog", { name: "导出当前页" }),
    ).toBeInTheDocument();
  });

  it("disables printing and explains why when permission restrictions deny it", async () => {
    installDocumentBackend({
      open_document: () => ({
        ...documentView,
        permissions: { ...allGrantedPermissions, canPrint: false },
      }),
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    expect(screen.getByRole("button", { name: "打印…" })).toBeDisabled();
    expect(screen.getByText("文档所有者限制了打印权限。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "导出当前页…" })).toBeEnabled();

    await waitFor(() => expect(menuHandler).toBeDefined());
    act(() => menuHandler?.({ payload: "file.print" }));
    expect(screen.queryByRole("dialog", { name: "打印" })).not.toBeInTheDocument();
  });
});

describe("page operations", () => {
  it("rotates the current page and refreshes metadata, canvas and thumbnails", async () => {
    let history = idleHistory;
    installDocumentBackend({
      edit_history: () => history,
      rotate_page: (args) => {
        history = dirtyHistory;
        return {
          pageCount: 3,
          pageIndex: (args as { pageIndex: number }).pageIndex,
          history,
        };
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await waitFor(() => expect(thumbnailCalls().length).toBeGreaterThan(0));
    const infoCalls = pageInfoCalls().length;
    const renderCalls = renderPageCalls().length;
    const thumbCalls = thumbnailCalls().length;

    await user.click(screen.getByRole("button", { name: "向右旋转" }));

    await waitFor(() =>
      expect(commandCalls("rotate_page")).toEqual([
        [
          "rotate_page",
          { documentId: "doc-1", pageIndex: 0, deltaDegrees: 90 },
        ],
      ]),
    );
    await waitFor(() =>
      expect(pageInfoCalls().length).toBeGreaterThan(infoCalls),
    );
    await waitFor(() =>
      expect(renderPageCalls().length).toBeGreaterThan(renderCalls),
    );
    await waitFor(() =>
      expect(thumbnailCalls().length).toBeGreaterThan(thumbCalls),
    );
    expect(await screen.findByText("未保存")).toBeInTheDocument();
  });

  it("deletes the current page, clamps navigation and refreshes the page count", async () => {
    let history = idleHistory;
    installDocumentBackend({
      edit_history: () => history,
      delete_page: () => {
        history = dirtyHistory;
        return { pageCount: 2, pageIndex: 1, history };
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await user.click(await screen.findByRole("button", { name: "第 3 页" }));
    expect(await screen.findByText("第 3 / 3 页 · 100%")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "删除页面" }));
    const dialog = await screen.findByRole("dialog", { name: "删除页面" });
    expect(within(dialog).getByText(/确定删除第 3 页/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "删除页面" }));

    await waitFor(() =>
      expect(commandCalls("delete_page")).toEqual([
        ["delete_page", { documentId: "doc-1", pageIndex: 2 }],
      ]),
    );
    expect(await screen.findByText("第 2 / 2 页 · 100%")).toBeInTheDocument();
    await waitFor(() =>
      expect(pageInfoCalls().some((call) => call.pageIndex === 1)).toBe(true),
    );
    await waitFor(() => expect(lastMenuState().get("page.next")).toBe(false));
  });

  it("inserts a blank page with the chosen paper size and selects it", async () => {
    let history = idleHistory;
    const inserted: unknown[] = [];
    installDocumentBackend({
      edit_history: () => history,
      insert_blank_page: (args) => {
        inserted.push(args);
        history = dirtyHistory;
        return { pageCount: 4, pageIndex: 1, history };
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await waitFor(() => expect(pageInfoCalls().length).toBeGreaterThan(0));

    await user.click(screen.getByRole("button", { name: "插入空白页…" }));
    const dialog = await screen.findByRole("dialog", { name: "插入空白页" });
    expect(
      within(dialog).getByRole("radio", { name: /与当前页相同/ }),
    ).toBeChecked();
    await user.click(within(dialog).getByRole("radio", { name: /A4/ }));
    await user.click(within(dialog).getByRole("button", { name: "插入" }));

    await waitFor(() =>
      expect(inserted).toEqual([
        {
          documentId: "doc-1",
          pageIndex: 1,
          size: { width: 595.28, height: 841.89 },
        },
      ]),
    );
    expect(await screen.findByText("第 2 / 4 页 · 100%")).toBeInTheDocument();
    await waitFor(() =>
      expect(pageInfoCalls().some((call) => call.pageIndex === 1)).toBe(true),
    );
    expect(await screen.findByText("未保存")).toBeInTheDocument();
  });

  it("disables page operations on a read-only document without touching history", async () => {
    installDocumentBackend({
      open_document: () => ({
        ...documentView,
        readOnly: true,
        permissions: {
          ...allGrantedPermissions,
          canModifyContent: false,
        },
      }),
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    expect(
      await screen.findByText("文档为只读，页面操作已禁用。"),
    ).toBeInTheDocument();
    for (const name of ["向左旋转", "向右旋转", "插入空白页…", "删除页面"]) {
      expect(screen.getByRole("button", { name })).toBeDisabled();
    }
    expect(screen.getByRole("button", { name: "第 1 页" })).toHaveAttribute(
      "draggable",
      "false",
    );
    expect(commandCalls("rotate_page")).toHaveLength(0);
    expect(commandCalls("delete_page")).toHaveLength(0);
    expect(commandCalls("insert_blank_page")).toHaveLength(0);
    expect(commandCalls("move_page")).toHaveLength(0);
  });

  it("undoes a page operation and restores the previous page count", async () => {
    const undoable = {
      canUndo: true,
      canRedo: false,
      revision: 1,
      savedRevision: 0,
      dirty: true,
    };
    const undone = {
      canUndo: false,
      canRedo: true,
      revision: 0,
      savedRevision: 0,
      dirty: false,
    };
    let history = undoable;
    installDocumentBackend({
      edit_history: () => history,
      undo_text_edit: () => {
        history = undone;
        return {
          pageIndex: 0,
          pageCount: 2,
          pageStructureChanged: true,
          history,
        };
      },
    });
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
    await waitFor(() => expect(lastMenuState().get("edit.undo")).toBe(true));
    await waitFor(() => expect(menuHandler).toBeDefined());
    const infoCalls = pageInfoCalls().length;

    act(() => menuHandler?.({ payload: "edit.undo" }));

    await waitFor(() =>
      expect(commandCalls("undo_text_edit")).toHaveLength(1),
    );
    expect(await screen.findByText("第 1 / 2 页 · 100%")).toBeInTheDocument();
    await waitFor(() =>
      expect(pageInfoCalls().length).toBeGreaterThan(infoCalls),
    );
    await waitFor(() => expect(lastMenuState().get("edit.redo")).toBe(true));
  });
});

describe("recovery and recent files", () => {
  const recoveryCandidate = {
    formatVersion: 1,
    sessionId: "s123-1700000000000-1",
    sourcePath: "/tmp/report.pdf",
    outputPath: "/tmp/copy.pdf",
    displayName: "report.pdf",
    sourceEncrypted: false,
    sourceReadOnly: false,
    sourcePermissions: allGrantedPermissions,
    dirty: true,
    revision: 3,
    savedRevision: 1,
    createdAtMillis: 1_700_000_000_000,
    appVersion: "0.1.0",
  };

  it("offers recovery at startup and restores the unsaved document", async () => {
    installDocumentBackend({
      recovery_candidates: () => [recoveryCandidate],
      restore_recovery: () => ({
        ...documentView,
        recovered: true,
        outputPath: "/tmp/copy.pdf",
      }),
      edit_history: () => dirtyHistory,
    });
    const user = userEvent.setup();

    render(<App />);

    const dialog = await screen.findByRole("dialog", {
      name: "恢复未保存的文档？",
    });
    expect(within(dialog).getByText("report.pdf")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "稍后处理" })).toHaveFocus();
    // The unanswered recovery question is modal: document/new-file actions in
    // the native menu stay unavailable until it is answered.
    await waitFor(() => expect(lastMenuState().get("file.open")).toBe(false));
    expect(commandCalls("open_document")).toHaveLength(0);

    await user.click(
      within(dialog).getByRole("button", { name: "恢复 report.pdf" }),
    );

    await waitFor(() => {
      expect(commandCalls("restore_recovery")).toEqual([
        ["restore_recovery", { sessionId: recoveryCandidate.sessionId }],
      ]);
    });
    await screen.findByText("report.pdf");
    expect(
      screen.queryByRole("dialog", { name: "恢复未保存的文档？" }),
    ).not.toBeInTheDocument();
    expect(screen.getByText("已恢复")).toBeInTheDocument();
    expect(commandCalls("open_document")).toHaveLength(0);
    await waitFor(() =>
      expect(setTitle).toHaveBeenLastCalledWith("report.pdf（未保存） — PDFX"),
    );
  });

  it("discloses that an encrypted source's recovery copy is unencrypted", async () => {
    installDocumentBackend({
      recovery_candidates: () => [
        { ...recoveryCandidate, sourceEncrypted: true },
      ],
    });

    render(<App />);

    const dialog = await screen.findByRole("dialog", {
      name: "恢复未保存的文档？",
    });
    expect(within(dialog).getByText(/恢复副本未加密/)).toBeInTheDocument();
  });

  it("ignores a dropped file while the recovery question is open", async () => {
    installDocumentBackend({
      recovery_candidates: () => [recoveryCandidate],
    });

    render(<App />);

    await screen.findByRole("dialog", { name: "恢复未保存的文档？" });
    await waitFor(() => expect(dragDropHandler).toBeDefined());

    act(() => {
      dragDropHandler?.({
        payload: {
          type: "drop",
          paths: ["/tmp/dropped.pdf"],
          position: { x: 0, y: 0 },
        },
      });
    });
    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(commandCalls("open_document")).toHaveLength(0);
    expect(
      screen.getByRole("dialog", { name: "恢复未保存的文档？" }),
    ).toBeInTheDocument();
  });

  it("skips the start-up recovery scan in a window opened for one document", async () => {
    installDocumentBackend({
      recovery_candidates: () => [recoveryCandidate],
      list_recent_files: () => recentFiles,
    });
    window.history.replaceState({}, "", "/?open=%2Ftmp%2Freport.pdf");

    render(<App />);

    expect(await screen.findByText("report.pdf")).toBeInTheDocument();
    expect(commandCalls("recovery_candidates")).toHaveLength(0);
    expect(
      screen.queryByRole("dialog", { name: "恢复未保存的文档？" }),
    ).not.toBeInTheDocument();
  });

  it("discards a recovery file and keeps the empty workspace", async () => {
    installDocumentBackend({
      recovery_candidates: () => [recoveryCandidate],
      discard_recovery: () => undefined,
    });
    const user = userEvent.setup();

    render(<App />);

    const dialog = await screen.findByRole("dialog", {
      name: "恢复未保存的文档？",
    });
    await user.click(
      within(dialog).getByRole("button", { name: "丢弃 report.pdf" }),
    );

    await waitFor(() => {
      expect(commandCalls("discard_recovery")).toEqual([
        ["discard_recovery", { sessionId: recoveryCandidate.sessionId }],
      ]);
    });
    expect(
      screen.queryByRole("dialog", { name: "恢复未保存的文档？" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("heading", { level: 2, name: "空工作区" }),
    ).toBeInTheDocument();
    expect(commandCalls("open_document")).toHaveLength(0);
  });

  const recentFiles = [
    {
      path: "/tmp/report.pdf",
      name: "report.pdf",
      lastOpenedMillis: 1_700_000_000_000,
    },
    {
      path: "/tmp/notes.pdf",
      name: "notes.pdf",
      lastOpenedMillis: 1_699_000_000_000,
    },
  ];

  it("lists recent files and opens one on demand without auto-opening", async () => {
    installDocumentBackend({ list_recent_files: () => recentFiles });
    const user = userEvent.setup();

    render(<App />);

    expect(
      await screen.findByRole("heading", { level: 3, name: "最近文件" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "打开 report.pdf" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "打开 notes.pdf" }),
    ).toBeInTheDocument();
    // Listing the recent files never opens a document by itself.
    expect(commandCalls("open_document")).toHaveLength(0);

    await user.click(screen.getByRole("button", { name: "打开 report.pdf" }));

    await waitFor(() => {
      expect(commandCalls("open_document")).toEqual([
        ["open_document", { path: "/tmp/report.pdf", password: null }],
      ]);
    });
    await screen.findByText("report.pdf");
    expect(
      screen.queryByRole("heading", { level: 3, name: "最近文件" }),
    ).not.toBeInTheDocument();
  });

  it("reports a missing recent file and removes it from the list", async () => {
    installDocumentBackend({
      list_recent_files: () => [recentFiles[0]],
      open_document: () => {
        throw { code: "file_not_found", message: "no such file" };
      },
      forget_recent_file: () => [],
    });
    const user = userEvent.setup();

    render(<App />);

    await user.click(
      await screen.findByRole("button", { name: "打开 report.pdf" }),
    );

    expect(
      await screen.findByText("找不到文件，它可能已被移动或删除。"),
    ).toBeInTheDocument();
    await waitFor(() => {
      expect(commandCalls("forget_recent_file")).toEqual([
        ["forget_recent_file", { path: "/tmp/report.pdf" }],
      ]);
    });
    expect(
      screen.queryByRole("button", { name: "打开 report.pdf" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("heading", { level: 2, name: "空工作区" }),
    ).toBeInTheDocument();
  });
});
