import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  AnnotationView,
  EditHistoryView,
  TextLayerView,
} from "../lib/backend";
import { PageTextLayer } from "./PageTextLayer";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const invokeMock = vi.mocked(invoke);

const textLayer: TextLayerView = {
  pageIndex: 0,
  pageWidth: 612,
  pageHeight: 792,
  rotation: 0,
  cropX: 0,
  cropY: 0,
  hasText: true,
  text: "Hello",
  chars: [],
};

const scannedLayer: TextLayerView = {
  ...textLayer,
  hasText: false,
  text: "",
};

const idleHistory: EditHistoryView = {
  canUndo: false,
  canRedo: false,
  revision: 0,
  savedRevision: 0,
  dirty: false,
};

const dirtyHistory: EditHistoryView = {
  ...idleHistory,
  canUndo: true,
  revision: 1,
  dirty: true,
};

const note: AnnotationView = {
  name: "note-1",
  pageIndex: 0,
  kind: "note",
  author: "作者",
  createdAt: "D:20261028120000Z",
  color: "#f9a825",
  opacity: 100,
  lineWidth: null,
  text: "记得回来",
  strokes: [],
  rects: [{ x: 80, y: 90, width: 30, height: 30 }],
  shape: null,
  stamp: null,
};

function mockCommands(
  overrides: Record<string, (args: Record<string, unknown>) => unknown> = {},
) {
  const handlers: Record<string, (args: Record<string, unknown>) => unknown> = {
    extract_text_layer: () => textLayer,
    edit_history: () => idleHistory,
    create_note_annotation: () => ({
      annotation: note,
      pageIndex: 0,
      history: dirtyHistory,
    }),
    create_ink_annotation: () => ({
      annotation: { ...note, kind: "ink", text: null, lineWidth: 3 },
      pageIndex: 0,
      history: dirtyHistory,
    }),
    transform_note_annotation: () => ({
      annotation: note,
      pageIndex: 0,
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
      pageIndex={0}
      canAnnotate
      {...props}
    />,
  );
  return screen.findByRole("region", { name: "文本层" });
}

describe("PageTextLayer notes and ink", () => {
  it("places a note where the page was clicked and selects it", async () => {
    mockCommands();
    const onSelectAnnotation = vi.fn();
    const region = await renderLayer({ onSelectAnnotation });

    fireEvent.click(screen.getByRole("button", { name: "便签" }));
    fireEvent.pointerDown(region, {
      clientX: 100,
      clientY: 200,
      button: 0,
      pointerId: 1,
    });

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("create_note_annotation", {
        documentId: "doc-1",
        request: {
          pageIndex: 0,
          color: "#f9a825",
          opacity: 100,
          text: "",
          rect: { x: 86, y: 186, width: 28, height: 28 },
        },
      });
    });
    expect(onSelectAnnotation).toHaveBeenCalledWith(note);
    expect(
      screen.getByRole("button", { name: "便签" }),
    ).toHaveAttribute("aria-pressed", "false");
  });

  it("moves and resizes an existing note as a single transform", async () => {
    mockCommands();
    const onSelectAnnotation = vi.fn();
    await renderLayer({
      annotations: [note],
      selectedAnnotation: note,
      onSelectAnnotation,
    });

    const noteElement = screen.getByRole("button", {
      name: "便签：记得回来",
    });
    fireEvent.pointerDown(noteElement, {
      clientX: 90,
      clientY: 100,
      button: 0,
      pointerId: 2,
    });
    fireEvent.pointerMove(noteElement, {
      clientX: 130,
      clientY: 140,
      pointerId: 2,
    });
    fireEvent.pointerUp(noteElement, {
      clientX: 130,
      clientY: 140,
      pointerId: 2,
    });

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("transform_note_annotation", {
        documentId: "doc-1",
        name: "note-1",
        rect: { x: 120, y: 130, width: 30, height: 30 },
      });
    });
    expect(onSelectAnnotation).toHaveBeenCalledWith(note);

    invokeMock.mockClear();
    const handle = screen.getByRole("button", { name: "调整便签大小" });
    fireEvent.pointerDown(handle, {
      clientX: 110,
      clientY: 120,
      button: 0,
      pointerId: 3,
    });
    fireEvent.pointerMove(handle, {
      clientX: 134,
      clientY: 132,
      pointerId: 3,
    });
    fireEvent.pointerUp(handle, {
      clientX: 134,
      clientY: 132,
      pointerId: 3,
    });

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("transform_note_annotation", {
        documentId: "doc-1",
        name: "note-1",
        rect: { x: 80, y: 90, width: 54, height: 42 },
      });
    });
  });

  it("draws one freehand stroke with the chosen colour, opacity and width", async () => {
    mockCommands();
    const region = await renderLayer();

    fireEvent.click(screen.getByRole("button", { name: "手绘" }));
    fireEvent.change(screen.getByLabelText("画笔颜色"), {
      target: { value: "#123456" },
    });
    fireEvent.change(screen.getByLabelText("画笔透明度"), {
      target: { value: "60" },
    });
    fireEvent.change(screen.getByLabelText("画笔线宽"), {
      target: { value: "8" },
    });

    fireEvent.pointerDown(region, {
      clientX: 50,
      clientY: 60,
      button: 0,
      pointerId: 4,
    });
    expect(screen.getByTestId("ink-draft")).toBeInTheDocument();
    fireEvent.pointerMove(region, {
      clientX: 100,
      clientY: 120,
      pointerId: 4,
    });
    fireEvent.pointerMove(region, {
      clientX: 150,
      clientY: 120,
      pointerId: 4,
    });
    fireEvent.pointerUp(region, {
      clientX: 150,
      clientY: 120,
      pointerId: 4,
    });

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("create_ink_annotation", {
        documentId: "doc-1",
        request: {
          pageIndex: 0,
          color: "#123456",
          opacity: 60,
          lineWidth: 8,
          strokes: [
            [
              { x: 50, y: 60 },
              { x: 100, y: 120 },
              { x: 150, y: 120 },
            ],
          ],
        },
      });
    });
    expect(screen.queryByTestId("ink-draft")).not.toBeInTheDocument();
  });

  it("offers note and ink tools on a scanned page when annotations are allowed", async () => {
    mockCommands({ extract_text_layer: () => scannedLayer });
    await renderLayer();

    expect(screen.getByText("此页没有可编辑文字")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "便签" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "手绘" })).toBeEnabled();
  });

  it("disables note and ink tools for read-only documents", async () => {
    mockCommands();
    await renderLayer({ readOnly: true });

    expect(screen.getByRole("button", { name: "便签" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "手绘" })).toBeDisabled();
  });
});
