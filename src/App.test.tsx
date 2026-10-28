import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

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
    close_document: () => undefined,
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

function setViewportSize(width: number, height: number) {
  const viewport = screen.getByLabelText("页面视口");
  Object.defineProperty(viewport, "clientWidth", { configurable: true, value: width });
  Object.defineProperty(viewport, "clientHeight", { configurable: true, value: height });
}

let setTitle: ReturnType<typeof vi.fn>;
let onCloseRequested: ReturnType<typeof vi.fn>;
let onDragDropEvent: ReturnType<typeof vi.fn>;
let setFullscreenMock: ReturnType<typeof vi.fn>;
let isFullscreenMock: ReturnType<typeof vi.fn>;
let dragDropHandler: ((event: unknown) => void) | undefined;
let menuHandler: ((event: unknown) => void) | undefined;

beforeEach(() => {
  setTitle = vi.fn().mockResolvedValue(undefined);
  onCloseRequested = vi.fn().mockResolvedValue(() => {});
  setFullscreenMock = vi.fn().mockResolvedValue(undefined);
  isFullscreenMock = vi.fn().mockResolvedValue(false);
  dragDropHandler = undefined;
  menuHandler = undefined;
  onDragDropEvent = vi.fn(async (handler: (event: unknown) => void) => {
    dragDropHandler = handler;
    return () => {};
  });

  vi.mocked(getCurrentWindow).mockReturnValue({
    setTitle,
    onCloseRequested,
    setFullscreen: setFullscreenMock,
    isFullscreen: isFullscreenMock,
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
});
