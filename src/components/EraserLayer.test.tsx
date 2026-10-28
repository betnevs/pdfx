import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  deleteAnnotation,
  eraseInkStroke,
  type AnnotationView,
  type EditHistoryView,
} from "../lib/backend";
import { EraserLayer, type EraserLayerProps } from "./EraserLayer";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../lib/backend", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/backend")>();
  return {
    ...actual,
    deleteAnnotation: vi.fn(),
    eraseInkStroke: vi.fn(),
  };
});

const deleteMock = vi.mocked(deleteAnnotation);
const eraseMock = vi.mocked(eraseInkStroke);

const history: EditHistoryView = {
  canUndo: true,
  canRedo: false,
  revision: 2,
  savedRevision: 0,
  dirty: true,
};

function base(overrides: Partial<AnnotationView>): AnnotationView {
  return {
    name: "a1",
    pageIndex: 0,
    kind: "highlight",
    author: null,
    createdAt: null,
    color: "#ffd400",
    opacity: 100,
    lineWidth: null,
    text: null,
    rects: [],
    strokes: [],
    shape: null,
    stamp: null,
    freeText: null,
    ...overrides,
  };
}

const ink: AnnotationView = base({
  name: "ink-1",
  kind: "ink",
  color: "#c62828",
  lineWidth: 6,
  strokes: [
    [
      { x: 100, y: 300 },
      { x: 160, y: 300 },
    ],
    [
      { x: 100, y: 360 },
      { x: 160, y: 360 },
    ],
  ],
  rects: [{ x: 97, y: 297, width: 66, height: 66 }],
});

const note: AnnotationView = base({
  name: "note-1",
  kind: "note",
  text: "便签",
  rects: [{ x: 40, y: 40, width: 30, height: 30 }],
});

function renderLayer(overrides: Partial<EraserLayerProps> = {}) {
  const onSelectAnnotation = vi.fn();
  const onChanged = vi.fn();
  render(
    <EraserLayer
      documentId="doc-1"
      pageIndex={0}
      pageWidth={612}
      pageHeight={792}
      readOnly={false}
      canAnnotate
      annotations={[ink, note]}
      selectedName={null}
      onSelectAnnotation={onSelectAnnotation}
      onChanged={onChanged}
      {...overrides}
    />,
  );
  return { onSelectAnnotation, onChanged };
}

function activate() {
  fireEvent.click(screen.getByRole("button", { name: "橡皮擦" }));
}

function eraseAt(x: number, y: number, extra: { shiftKey?: boolean } = {}) {
  fireEvent.pointerDown(screen.getByTestId("eraser-surface"), {
    clientX: x,
    clientY: y,
    button: 0,
    ...extra,
  });
}

beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
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
  eraseMock.mockResolvedValue({
    annotation: { ...ink, strokes: [ink.strokes[0]] },
    pageIndex: 0,
    history,
  });
  deleteMock.mockResolvedValue({
    annotation: null,
    pageIndex: 0,
    history,
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  deleteMock.mockReset();
  eraseMock.mockReset();
});

describe("EraserLayer", () => {
  it("hides the eraser entirely when annotations cannot be changed", () => {
    const { rerender } = render(
      <EraserLayer
        documentId="doc-1"
        pageIndex={0}
        pageWidth={612}
        pageHeight={792}
        readOnly={false}
        canAnnotate={false}
        annotations={[ink]}
        selectedName={null}
        onSelectAnnotation={vi.fn()}
        onChanged={vi.fn()}
      />,
    );
    expect(
      screen.queryByRole("button", { name: "橡皮擦" }),
    ).not.toBeInTheDocument();
    rerender(
      <EraserLayer
        documentId="doc-1"
        pageIndex={0}
        pageWidth={612}
        pageHeight={792}
        readOnly
        canAnnotate
        annotations={[ink]}
        selectedName={null}
        onSelectAnnotation={vi.fn()}
        onChanged={vi.fn()}
      />,
    );
    expect(
      screen.queryByRole("button", { name: "橡皮擦" }),
    ).not.toBeInTheDocument();
  });

  it("arms and disarms the eraser surface from its toolbar", () => {
    renderLayer();
    expect(screen.queryByTestId("eraser-surface")).not.toBeInTheDocument();
    activate();
    expect(screen.getByTestId("eraser-surface")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "橡皮擦" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    activate();
    expect(screen.queryByTestId("eraser-surface")).not.toBeInTheDocument();
  });

  it("removes only the hit ink stroke through erase_ink_stroke", async () => {
    const { onChanged, onSelectAnnotation } = renderLayer({
      selectedName: "ink-1",
    });
    activate();
    eraseAt(130, 305);
    await waitFor(() =>
      expect(eraseMock).toHaveBeenCalledWith("doc-1", "ink-1", 0),
    );
    expect(deleteMock).not.toHaveBeenCalled();
    expect(onChanged).toHaveBeenCalledTimes(1);
    // The refreshed annotation keeps the same identity and is re-selected.
    expect(onSelectAnnotation).toHaveBeenCalledWith(
      expect.objectContaining({ name: "ink-1" }),
    );
  });

  it("deletes the whole ink annotation on Shift-click", async () => {
    const { onChanged, onSelectAnnotation } = renderLayer({
      selectedName: "ink-1",
    });
    activate();
    eraseAt(130, 305, { shiftKey: true });
    await waitFor(() =>
      expect(deleteMock).toHaveBeenCalledWith("doc-1", "ink-1"),
    );
    expect(eraseMock).not.toHaveBeenCalled();
    expect(onChanged).toHaveBeenCalledTimes(1);
    expect(onSelectAnnotation).toHaveBeenCalledWith(null);
  });

  it("deletes a non-ink annotation under the pointer", async () => {
    const { onSelectAnnotation } = renderLayer({ selectedName: "note-1" });
    activate();
    eraseAt(55, 55);
    await waitFor(() =>
      expect(deleteMock).toHaveBeenCalledWith("doc-1", "note-1"),
    );
    expect(eraseMock).not.toHaveBeenCalled();
    expect(onSelectAnnotation).toHaveBeenCalledWith(null);
  });

  it("clears the selection when the last stroke deletes the annotation", async () => {
    eraseMock.mockResolvedValue({ annotation: null, pageIndex: 0, history });
    const { onSelectAnnotation } = renderLayer({ selectedName: "ink-1" });
    activate();
    eraseAt(130, 305);
    await waitFor(() => expect(eraseMock).toHaveBeenCalled());
    await waitFor(() => expect(onSelectAnnotation).toHaveBeenCalledWith(null));
  });

  it("reports a miss without touching the document", async () => {
    renderLayer();
    activate();
    eraseAt(500, 700);
    await waitFor(() =>
      expect(screen.getByText("此处没有可擦除的标注")).toBeInTheDocument(),
    );
    expect(eraseMock).not.toHaveBeenCalled();
    expect(deleteMock).not.toHaveBeenCalled();
  });

  it("shows the command error when erasing is refused", async () => {
    eraseMock.mockRejectedValue({
      code: "permission_denied",
      message: "文档权限不允许修改批注",
    });
    const { onChanged } = renderLayer();
    activate();
    eraseAt(130, 305);
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(
        "文档权限不允许修改批注",
      ),
    );
    expect(onChanged).not.toHaveBeenCalled();
  });

  it("uses the configured eraser radius", async () => {
    renderLayer();
    activate();
    fireEvent.change(screen.getByLabelText("橡皮擦半径"), {
      target: { value: "30" },
    });
    eraseAt(130, 320);
    await waitFor(() =>
      expect(eraseMock).toHaveBeenCalledWith("doc-1", "ink-1", 0),
    );
  });

  it("only hit-tests annotations belonging to this page", async () => {
    renderLayer({
      pageIndex: 1,
      annotations: [
        { ...note, name: "other-page", pageIndex: 0 },
        { ...note, name: "this-page", pageIndex: 1 },
      ],
    });
    activate();
    eraseAt(55, 55);
    await waitFor(() =>
      expect(deleteMock).toHaveBeenCalledWith("doc-1", "this-page"),
    );
  });
});
