import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const openDialogMock = vi.mocked(openDialog);
const listenMock = vi.mocked(listen);

const documentView = {
  documentId: "doc-1",
  name: "report.pdf",
  path: "/tmp/report.pdf",
  pageCount: 3,
  pageWidth: 612,
  pageHeight: 792,
};

const pageImage = {
  documentId: "doc-1",
  pageIndex: 0,
  scale: 1,
  width: 612,
  height: 792,
  pngBase64: "iVBORw0KGgo=",
};

type CommandHandler = (args: unknown) => unknown;

function installBackend(overrides: Record<string, CommandHandler> = {}) {
  invokeMock.mockImplementation(async (command: string, args?: unknown) => {
    if (command === "app_info") {
      return { name: "PDFX", version: "0.1.0" };
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
    render_page: () => pageImage,
    close_document: () => undefined,
    ...overrides,
  });
}

function renderPageCalls(): unknown[] {
  return invokeMock.mock.calls
    .filter(([command]) => command === "render_page")
    .map(([, args]) => args);
}

function commandCalls(command: string) {
  return invokeMock.mock.calls.filter(([name]) => name === command);
}

let setTitle: ReturnType<typeof vi.fn>;
let onCloseRequested: ReturnType<typeof vi.fn>;
let onDragDropEvent: ReturnType<typeof vi.fn>;
let dragDropHandler: ((event: unknown) => void) | undefined;
let menuHandler: ((event: unknown) => void) | undefined;

beforeEach(() => {
  setTitle = vi.fn().mockResolvedValue(undefined);
  onCloseRequested = vi.fn().mockResolvedValue(() => {});
  dragDropHandler = undefined;
  menuHandler = undefined;
  onDragDropEvent = vi.fn(async (handler: (event: unknown) => void) => {
    dragDropHandler = handler;
    return () => {};
  });

  vi.mocked(getCurrentWindow).mockReturnValue({
    setTitle,
    onCloseRequested,
  } as unknown as ReturnType<typeof getCurrentWindow>);
  vi.mocked(getCurrentWebview).mockReturnValue({
    onDragDropEvent,
  } as unknown as ReturnType<typeof getCurrentWebview>);
  listenMock.mockImplementation(async (_event, handler) => {
    menuHandler = handler as (event: unknown) => void;
    return () => {};
  });
  openDialogMock.mockResolvedValue(null);
  window.history.replaceState({}, "", "/");
});

afterEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(getCurrentWindow).mockReset();
  vi.mocked(getCurrentWebview).mockReset();
  listenMock.mockReset();
  openDialogMock.mockReset();
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
    expect(screen.getByRole("img", { name: "第 1 页" })).toHaveAttribute("width", "612");
    expect(screen.getByText("第 1 / 3 页 · 100%")).toBeInTheDocument();
    expect(openDialogMock).toHaveBeenCalledWith(
      expect.objectContaining({
        multiple: false,
        filters: [{ name: "PDF", extensions: ["pdf"] }],
      }),
    );
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

  it("wires native menu ids to open and zoom actions", async () => {
    installDocumentBackend();
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");
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

    openDialogMock.mockResolvedValueOnce(null);
    act(() => menuHandler?.({ payload: "file.open" }));
    await waitFor(() => expect(openDialogMock).toHaveBeenCalledTimes(2));
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

  it("registers a close guard that releases the document when it is not dirty", async () => {
    installDocumentBackend();
    openDialogMock.mockResolvedValueOnce("/tmp/report.pdf");
    const user = userEvent.setup();

    render(<App />);
    await user.click(screen.getByRole("button", { name: "打开 PDF…" }));
    await screen.findByText("report.pdf");

    await waitFor(() => expect(onCloseRequested).toHaveBeenCalled());
    const handler = onCloseRequested.mock.calls[0][0] as (event: {
      preventDefault: () => void;
    }) => void;
    const preventDefault = vi.fn();

    handler({ preventDefault });

    expect(preventDefault).not.toHaveBeenCalled();
    expect(commandCalls("close_document")).toEqual([
      ["close_document", { documentId: "doc-1" }],
    ]);
  });
});
