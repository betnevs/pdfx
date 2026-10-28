import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  createStampAnnotation,
  transformStampAnnotation,
  type AnnotationView,
  type EditHistoryView,
} from "../lib/backend";
import { StampLayer, type StampLayerProps } from "./StampLayer";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../lib/backend", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/backend")>();
  return {
    ...actual,
    createStampAnnotation: vi.fn(),
    transformStampAnnotation: vi.fn(),
  };
});

const createMock = vi.mocked(createStampAnnotation);
const transformMock = vi.mocked(transformStampAnnotation);

const idleHistory: EditHistoryView = {
  canUndo: false,
  canRedo: false,
  revision: 0,
  savedRevision: 0,
  dirty: false,
};

const approved: AnnotationView = {
  name: "s1",
  pageIndex: 0,
  kind: "stamp",
  author: "测试作者",
  createdAt: "D:20261028123456Z",
  color: "#15803d",
  opacity: 100,
  rects: [{ x: 216, y: 366, width: 180, height: 60 }],
  stamp: {
    kind: "approved",
    frame: { x: 216, y: 366, width: 180, height: 60, rotation: 0 },
  },
};

function renderLayer(overrides: Partial<StampLayerProps> = {}) {
  const onSelect = vi.fn();
  const onPlaced = vi.fn();
  const onArmedChange = vi.fn();
  const onChanged = vi.fn();
  render(
    <StampLayer
      documentId="doc-1"
      pageIndex={0}
      pageWidth={612}
      pageHeight={792}
      readOnly={false}
      canAnnotate
      annotations={[]}
      selectedName={null}
      armed={null}
      onSelect={onSelect}
      onPlaced={onPlaced}
      onArmedChange={onArmedChange}
      onChanged={onChanged}
      {...overrides}
    />,
  );
  return { onSelect, onPlaced, onArmedChange, onChanged };
}

beforeEach(() => {
  // The layer is 612x792 display points in the test page.
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
});

afterEach(() => {
  vi.restoreAllMocks();
  createMock.mockReset();
  transformMock.mockReset();
});

describe("StampLayer", () => {
  it("places the armed stamp centred on the click and selects it", async () => {
    const created: AnnotationView = {
      ...approved,
      name: "s2",
      stamp: {
        kind: "confidential",
        frame: { x: 216, y: 366, width: 180, height: 60, rotation: 0 },
      },
    };
    createMock.mockResolvedValue({
      annotation: created,
      pageIndex: 0,
      history: idleHistory,
    });
    const { onPlaced, onArmedChange, onChanged } = renderLayer({
      armed: "confidential",
    });

    fireEvent.pointerDown(screen.getByTestId("stamp-layer"), {
      button: 0,
      clientX: 306,
      clientY: 396,
      pointerId: 1,
    });

    await waitFor(() => {
      expect(createMock).toHaveBeenCalledWith("doc-1", {
        pageIndex: 0,
        kind: "confidential",
        opacity: 100,
        frame: { x: 216, y: 366, width: 180, height: 60, rotation: 0 },
      });
    });
    expect(onChanged).toHaveBeenCalledTimes(1);
    expect(onArmedChange).toHaveBeenCalledWith(null);
    expect(onPlaced).toHaveBeenCalledWith(created);
  });

  it("does not place a stamp without annotation permission", () => {
    renderLayer({ armed: "approved", canAnnotate: false });

    fireEvent.pointerDown(screen.getByTestId("stamp-layer"), {
      button: 0,
      clientX: 306,
      clientY: 396,
      pointerId: 1,
    });

    expect(createMock).not.toHaveBeenCalled();
  });

  it("reports a failed placement without arming down", async () => {
    createMock.mockRejectedValue({
      code: "permission_denied",
      message: "不允许放置图章",
    });
    const { onPlaced, onArmedChange } = renderLayer({ armed: "approved" });

    fireEvent.pointerDown(screen.getByTestId("stamp-layer"), {
      button: 0,
      clientX: 306,
      clientY: 396,
      pointerId: 1,
    });

    expect(await screen.findByRole("alert")).toHaveTextContent("不允许放置图章");
    expect(onPlaced).not.toHaveBeenCalled();
    expect(onArmedChange).not.toHaveBeenCalled();
  });

  it("selects a stamp and commits a drag as one transform", async () => {
    transformMock.mockResolvedValue({
      annotation: approved,
      pageIndex: 0,
      history: idleHistory,
    });
    const { onSelect, onChanged } = renderLayer({ annotations: [approved] });

    fireEvent.pointerDown(screen.getByTestId("stamp-hit-s1"), {
      button: 0,
      clientX: 250,
      clientY: 430,
      pointerId: 2,
    });
    fireEvent.pointerMove(screen.getByTestId("stamp-layer"), {
      clientX: 290,
      clientY: 450,
      pointerId: 2,
    });
    fireEvent.pointerUp(screen.getByTestId("stamp-layer"), {
      clientX: 290,
      clientY: 450,
      pointerId: 2,
    });

    expect(onSelect).toHaveBeenCalledWith(approved);
    await waitFor(() => {
      expect(transformMock).toHaveBeenCalledTimes(1);
    });
    expect(transformMock).toHaveBeenCalledWith("doc-1", {
      name: "s1",
      frame: {
        x: expect.closeTo(256, 5),
        y: expect.closeTo(386, 5),
        width: expect.closeTo(180, 5),
        height: expect.closeTo(60, 5),
        rotation: 0,
      },
    });
    expect(onChanged).toHaveBeenCalledTimes(1);
  });

  it("does not commit a click without movement", async () => {
    const { onSelect } = renderLayer({
      annotations: [approved],
      selectedName: "s1",
    });

    fireEvent.pointerDown(screen.getByTestId("stamp-hit-s1"), {
      button: 0,
      clientX: 250,
      clientY: 430,
      pointerId: 2,
    });
    fireEvent.pointerUp(screen.getByTestId("stamp-layer"), {
      clientX: 250,
      clientY: 430,
      pointerId: 2,
    });

    expect(onSelect).toHaveBeenCalledWith(approved);
    await Promise.resolve();
    expect(transformMock).not.toHaveBeenCalled();
  });

  it("resizes around the stamp centre from the resize handle", async () => {
    transformMock.mockResolvedValue({
      annotation: approved,
      pageIndex: 0,
      history: idleHistory,
    });
    renderLayer({ annotations: [approved], selectedName: "s1" });

    fireEvent.pointerDown(screen.getByTestId("stamp-resize-handle"), {
      button: 0,
      clientX: 396,
      clientY: 426,
      pointerId: 3,
    });
    fireEvent.pointerMove(screen.getByTestId("stamp-layer"), {
      clientX: 426,
      clientY: 456,
      pointerId: 3,
    });
    fireEvent.pointerUp(screen.getByTestId("stamp-layer"), {
      clientX: 426,
      clientY: 456,
      pointerId: 3,
    });

    await waitFor(() => {
      expect(transformMock).toHaveBeenCalledTimes(1);
    });
    expect(transformMock).toHaveBeenCalledWith("doc-1", {
      name: "s1",
      frame: {
        x: expect.closeTo(186, 5),
        y: expect.closeTo(336, 5),
        width: expect.closeTo(240, 5),
        height: expect.closeTo(120, 5),
        rotation: 0,
      },
    });
  });

  it("rotates from the rotate handle and snaps with Shift", async () => {
    transformMock.mockResolvedValue({
      annotation: approved,
      pageIndex: 0,
      history: idleHistory,
    });
    renderLayer({ annotations: [approved], selectedName: "s1" });

    fireEvent.pointerDown(screen.getByTestId("stamp-rotate-handle"), {
      button: 0,
      clientX: 306,
      clientY: 300,
      pointerId: 4,
    });
    fireEvent.pointerMove(screen.getByTestId("stamp-layer"), {
      clientX: 426,
      clientY: 396,
      pointerId: 4,
    });
    fireEvent.pointerUp(screen.getByTestId("stamp-layer"), {
      clientX: 426,
      clientY: 396,
      pointerId: 4,
      shiftKey: true,
    });

    await waitFor(() => {
      expect(transformMock).toHaveBeenCalledTimes(1);
    });
    expect(transformMock).toHaveBeenCalledWith("doc-1", {
      name: "s1",
      frame: { x: 216, y: 366, width: 180, height: 60, rotation: 90 },
    });
  });

  it("selects but does not transform stamps when the document is read-only", () => {
    const { onSelect } = renderLayer({
      annotations: [approved],
      selectedName: "s1",
      readOnly: true,
    });

    expect(screen.queryByTestId("stamp-rotate-handle")).not.toBeInTheDocument();
    expect(screen.queryByTestId("stamp-resize-handle")).not.toBeInTheDocument();

    fireEvent.pointerDown(screen.getByTestId("stamp-hit-s1"), {
      button: 0,
      clientX: 250,
      clientY: 430,
      pointerId: 5,
    });
    fireEvent.pointerMove(screen.getByTestId("stamp-layer"), {
      clientX: 290,
      clientY: 450,
      pointerId: 5,
    });
    fireEvent.pointerUp(screen.getByTestId("stamp-layer"), {
      clientX: 290,
      clientY: 450,
      pointerId: 5,
    });

    expect(onSelect).toHaveBeenCalledWith(approved);
    expect(transformMock).not.toHaveBeenCalled();
  });

  it("draws only stamps on its own page", () => {
    const otherPage: AnnotationView = {
      ...approved,
      name: "s9",
      pageIndex: 1,
    };
    const highlight: AnnotationView = {
      ...approved,
      name: "a1",
      kind: "highlight",
      stamp: null,
    };
    renderLayer({
      annotations: [approved, otherPage, highlight],
      selectedName: "s1",
    });

    expect(screen.getByTestId("stamp-hit-s1")).toBeInTheDocument();
    expect(screen.queryByTestId("stamp-hit-s9")).not.toBeInTheDocument();
    expect(screen.queryByTestId("stamp-hit-a1")).not.toBeInTheDocument();
  });
});
