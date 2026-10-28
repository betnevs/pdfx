import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { TextCharView, TextLayerView } from "../lib/backend";
import { PageTextLayer } from "./PageTextLayer";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const invokeMock = vi.mocked(invoke);

function makeChar(text: string, x: number, y: number, width: number): TextCharView {
  return {
    text,
    x,
    y,
    width,
    height: 20,
    originX: x,
    originY: y + 20,
    fontSize: 18,
    generated: false,
  };
}

const chars: TextCharView[] = [
  makeChar("H", 100, 100, 12),
  makeChar("i", 112, 100, 6),
  makeChar(" ", 118, 100, 6),
  makeChar("t", 124, 100, 6),
  makeChar("h", 130, 100, 8),
  makeChar("e", 138, 100, 8),
];

const textLayer: TextLayerView = {
  pageIndex: 2,
  pageWidth: 612,
  pageHeight: 792,
  rotation: 0,
  cropX: 0,
  cropY: 0,
  hasText: true,
  text: "Hi the",
  chars,
};

const emptyLayer: TextLayerView = {
  pageIndex: 2,
  pageWidth: 612,
  pageHeight: 792,
  rotation: 0,
  cropX: 0,
  cropY: 0,
  hasText: false,
  text: "",
  chars: [],
};

let clipboardWrite: ReturnType<typeof vi.fn>;
const originalGetBoundingClientRect = Element.prototype.getBoundingClientRect;

beforeEach(() => {
  clipboardWrite = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: clipboardWrite },
  });
  // The overlay maps client coordinates through its rendered size. jsdom has
  // no layout, so give every element the page-sized box.
  Element.prototype.getBoundingClientRect = () =>
    ({
      x: 0,
      y: 0,
      left: 0,
      top: 0,
      right: 612,
      bottom: 792,
      width: 612,
      height: 792,
      toJSON: () => ({}),
    }) as DOMRect;
});

afterEach(() => {
  invokeMock.mockReset();
  Element.prototype.getBoundingClientRect = originalGetBoundingClientRect;
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: undefined,
  });
});

describe("PageTextLayer", () => {
  it("fetches only its own page's text layer and exposes a transparent overlay", async () => {
    invokeMock.mockResolvedValue(textLayer);

    render(<PageTextLayer documentId="doc-1" pageIndex={2} />);

    const region = await screen.findByRole("region", { name: "文本层" });
    expect(invokeMock).toHaveBeenCalledWith("extract_text_layer", {
      documentId: "doc-1",
      pageIndex: 2,
    });
    expect(region).toHaveClass("page-text-layer");
    expect(region).not.toHaveTextContent("此页没有可编辑文字");
  });

  it("shows the scanned-page state instead of inventing text", async () => {
    invokeMock.mockResolvedValue(emptyLayer);

    render(<PageTextLayer documentId="doc-1" pageIndex={2} />);

    expect(await screen.findByText("此页没有可编辑文字")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "文本层" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "复制" })).not.toBeInTheDocument();
  });

  it("ignores a stale text layer response when the page changes", async () => {
    let resolveFirst: ((layer: TextLayerView) => void) | undefined;
    const first = new Promise<TextLayerView>((resolve) => {
      resolveFirst = resolve;
    });
    invokeMock.mockImplementation((_command, args) => {
      const pageIndex = (args as { pageIndex: number }).pageIndex;
      return pageIndex === 0
        ? first
        : Promise.resolve({ ...emptyLayer, pageIndex: 1 });
    });

    const { rerender } = render(<PageTextLayer documentId="doc-1" pageIndex={0} />);
    rerender(<PageTextLayer documentId="doc-1" pageIndex={1} />);

    expect(await screen.findByText("此页没有可编辑文字")).toBeInTheDocument();

    resolveFirst?.(textLayer);
    await waitFor(() => {
      expect(screen.getByText("此页没有可编辑文字")).toBeInTheDocument();
    });
    expect(screen.queryByRole("region", { name: "文本层" })).not.toBeInTheDocument();
  });

  it("positions highlights relative to the page box so zoom keeps alignment", async () => {
    invokeMock.mockResolvedValue(textLayer);

    render(<PageTextLayer documentId="doc-1" pageIndex={2} />);
    const region = await screen.findByRole("region", { name: "文本层" });

    fireEvent.pointerDown(region, { clientX: 102, clientY: 105, button: 0, pointerId: 1 });
    fireEvent.pointerUp(region, { clientX: 102, clientY: 105, pointerId: 1 });

    const highlight = screen.getByTestId("text-highlight");
    expect(parseFloat(highlight.style.left)).toBeCloseTo((100 / 612) * 100, 5);
    expect(parseFloat(highlight.style.top)).toBeCloseTo((100 / 792) * 100, 5);
    expect(parseFloat(highlight.style.width)).toBeCloseTo((12 / 612) * 100, 5);
    expect(parseFloat(highlight.style.height)).toBeCloseTo((20 / 792) * 100, 5);
  });

  it("selects characters by dragging and copies the selection", async () => {
    invokeMock.mockResolvedValue(textLayer);

    render(<PageTextLayer documentId="doc-1" pageIndex={2} />);
    const region = await screen.findByRole("region", { name: "文本层" });

    fireEvent.pointerDown(region, { clientX: 102, clientY: 105, button: 0, pointerId: 1 });
    fireEvent.pointerMove(region, { clientX: 145, clientY: 105, pointerId: 1 });
    fireEvent.pointerUp(region, { clientX: 145, clientY: 105, pointerId: 1 });

    expect(screen.getAllByTestId("text-highlight")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "复制" }));

    expect(clipboardWrite).toHaveBeenCalledWith("Hi the");
    expect(await screen.findByText("已复制")).toBeInTheDocument();
  });

  it("selects a word on double click and a line on triple click", async () => {
    invokeMock.mockResolvedValue(textLayer);

    render(<PageTextLayer documentId="doc-1" pageIndex={2} />);
    const region = await screen.findByRole("region", { name: "文本层" });

    fireEvent.doubleClick(region, { clientX: 114, clientY: 105, detail: 2 });
    fireEvent.click(screen.getByRole("button", { name: "复制" }));
    expect(clipboardWrite).toHaveBeenLastCalledWith("Hi");

    fireEvent.click(region, { clientX: 132, clientY: 105, detail: 3 });
    fireEvent.click(screen.getByRole("button", { name: "复制" }));
    expect(clipboardWrite).toHaveBeenLastCalledWith("Hi the");
  });

  it("clears the selection with Escape and copies with the keyboard", async () => {
    invokeMock.mockResolvedValue(textLayer);

    render(<PageTextLayer documentId="doc-1" pageIndex={2} />);
    const region = await screen.findByRole("region", { name: "文本层" });

    fireEvent.pointerDown(region, { clientX: 102, clientY: 105, button: 0, pointerId: 1 });
    fireEvent.pointerUp(region, { clientX: 102, clientY: 105, pointerId: 1 });
    expect(screen.getByRole("button", { name: "复制" })).toBeInTheDocument();

    fireEvent.keyDown(region, { key: "c", ctrlKey: true });
    await waitFor(() => expect(clipboardWrite).toHaveBeenCalledWith("H"));

    fireEvent.keyDown(region, { key: "Escape" });
    expect(screen.queryByRole("button", { name: "复制" })).not.toBeInTheDocument();
  });
});
