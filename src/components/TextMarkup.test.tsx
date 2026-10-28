import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  AnnotationView,
  EditHistoryView,
  TextCharView,
  TextLayerView,
} from "../lib/backend";
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

const textLayer: TextLayerView = {
  pageIndex: 2,
  pageWidth: 612,
  pageHeight: 792,
  rotation: 0,
  cropX: 0,
  cropY: 0,
  hasText: true,
  text: "Hi the",
  chars: [
    makeChar("H", 100, 100, 12),
    makeChar("i", 112, 100, 6),
    makeChar(" ", 118, 100, 6),
    makeChar("t", 124, 100, 6),
    makeChar("h", 130, 100, 8),
    makeChar("e", 138, 100, 8),
  ],
};

const idleHistory: EditHistoryView = {
  canUndo: false,
  canRedo: false,
  revision: 0,
  savedRevision: 0,
  dirty: false,
};

const dirtyHistory: EditHistoryView = {
  canUndo: true,
  canRedo: false,
  revision: 1,
  savedRevision: 0,
  dirty: true,
};

const created: AnnotationView = {
  name: "pdfx-1",
  pageIndex: 2,
  kind: "highlight",
  author: "测试作者",
  createdAt: "D:20261028123456Z",
  color: "#ffd400",
  opacity: 50,
  lineWidth: null,
  text: null,
  strokes: [],
  rects: [{ x: 100, y: 100, width: 12, height: 20 }],
  shape: null,
};

function mockCommands(overrides: Record<string, (args: Record<string, unknown>) => unknown> = {}) {
  const handlers: Record<string, (args: Record<string, unknown>) => unknown> = {
    extract_text_layer: () => textLayer,
    edit_history: () => idleHistory,
    create_text_markup_annotation: () => ({
      annotation: created,
      pageIndex: 2,
      history: dirtyHistory,
    }),
    ...overrides,
  };
  invokeMock.mockImplementation((command, args) => {
    const handler = handlers[command as string];
    if (!handler) {
      return Promise.reject(new Error(`unexpected command ${command as string}`));
    }
    return Promise.resolve(handler((args ?? {}) as Record<string, unknown>));
  });
}

const originalGetBoundingClientRect = Element.prototype.getBoundingClientRect;

beforeEach(() => {
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
});

async function renderLayer(
  props: Partial<Parameters<typeof PageTextLayer>[0]> = {},
) {
  render(
    <PageTextLayer
      documentId="doc-1"
      pageIndex={2}
      canAnnotate
      {...props}
    />,
  );
  await screen.findByRole("region", { name: "文本层" });
}

function selectFirstCharacter() {
  const region = screen.getByRole("region", { name: "文本层" });
  fireEvent.pointerDown(region, {
    clientX: 102,
    clientY: 105,
    button: 0,
    pointerId: 1,
  });
  fireEvent.pointerUp(region, { clientX: 102, clientY: 105, pointerId: 1 });
}

describe("PageTextLayer text markup", () => {
  it("highlights a selection with the chosen colour and opacity", async () => {
    mockCommands();
    await renderLayer();
    selectFirstCharacter();

    expect(screen.getByRole("button", { name: "高亮" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "下划线" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "删除线" })).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("注释颜色"), {
      target: { value: "#00ff00" },
    });
    fireEvent.change(screen.getByLabelText("注释透明度"), {
      target: { value: "70" },
    });
    fireEvent.click(screen.getByRole("button", { name: "高亮" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("create_text_markup_annotation", {
        documentId: "doc-1",
        request: {
          pageIndex: 2,
          kind: "highlight",
          color: "#00ff00",
          opacity: 70,
          rects: [{ x: 100, y: 100, width: 12, height: 20 }],
        },
      });
    });
    // The selection is consumed by the new annotation.
    expect(
      screen.queryByRole("button", { name: "高亮" }),
    ).not.toBeInTheDocument();
  });

  it.each([
    ["下划线", "underline"],
    ["删除线", "strikeout"],
  ])("creates a %s annotation", async (label, kind) => {
    mockCommands();
    await renderLayer();
    selectFirstCharacter();

    fireEvent.click(screen.getByRole("button", { name: label }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "create_text_markup_annotation",
        expect.objectContaining({
          request: expect.objectContaining({
            pageIndex: 2,
            kind,
            color: "#ffd400",
            opacity: 50,
          }),
        }),
      );
    });
  });

  it("reports why text markup is unavailable without annotation permission", async () => {
    mockCommands();
    await renderLayer({ canAnnotate: false });
    selectFirstCharacter();

    expect(screen.getByText("此文档不允许添加注释")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "高亮" })).not.toBeInTheDocument();
    expect(screen.queryByLabelText("注释颜色")).not.toBeInTheDocument();
  });

  it("keeps the selection and shows the backend error when markup fails", async () => {
    mockCommands({
      create_text_markup_annotation: () => {
        throw { code: "invalid_argument", message: "选区无效" };
      },
    });
    await renderLayer();
    selectFirstCharacter();
    fireEvent.click(screen.getByRole("button", { name: "高亮" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("选区无效");
    expect(screen.getByRole("button", { name: "高亮" })).toBeInTheDocument();
  });

  it("outlines the annotation selected in the list on this page", async () => {
    mockCommands();
    await renderLayer({ selectedAnnotation: created });

    const outline = screen.getByTestId("selected-annotation-outline");
    expect(outline).toHaveStyle({ borderColor: "#ffd400" });
    expect(parseFloat(outline.style.left)).toBeCloseTo((100 / 612) * 100, 5);
    expect(parseFloat(outline.style.top)).toBeCloseTo((100 / 792) * 100, 5);
    expect(parseFloat(outline.style.width)).toBeCloseTo((12 / 612) * 100, 5);
    expect(parseFloat(outline.style.height)).toBeCloseTo((20 / 792) * 100, 5);
  });

  it("does not draw another page's selected annotation", async () => {
    mockCommands();
    await renderLayer({ selectedAnnotation: { ...created, pageIndex: 0 } });

    expect(
      screen.queryByTestId("selected-annotation-outline"),
    ).not.toBeInTheDocument();
  });
});
