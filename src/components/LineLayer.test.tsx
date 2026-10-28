import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  CreateLineRequest,
  EditHistoryView,
  LineEditResultView,
  LineObjectView,
} from "../lib/backend";
import {
  createLineObject,
  deleteLineObject,
  pageLineObjects,
  transformLineObject,
  updateLineStyle,
} from "../lib/backend";
import { DOCUMENT_CHANGED_EVENT, notifyDocumentChanged } from "../lib/documentEvents";
import { LineLayer } from "./LineLayer";

vi.mock("../lib/backend", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/backend")>();
  return {
    ...actual,
    createLineObject: vi.fn(),
    deleteLineObject: vi.fn(),
    pageLineObjects: vi.fn(),
    transformLineObject: vi.fn(),
    updateLineStyle: vi.fn(),
  };
});

const createLineObjectMock = vi.mocked(createLineObject);
const deleteLineObjectMock = vi.mocked(deleteLineObject);
const pageLineObjectsMock = vi.mocked(pageLineObjects);
const transformLineObjectMock = vi.mocked(transformLineObject);
const updateLineStyleMock = vi.mocked(updateLineStyle);

const dirtyHistory: EditHistoryView = {
  canUndo: true,
  canRedo: false,
  revision: 1,
  savedRevision: 0,
  dirty: true,
};

const line: LineObjectView = {
  lineId: 1,
  pageIndex: 0,
  kind: "line",
  geometry: { x1: 100, y1: 300, x2: 300, y2: 300 },
  style: { strokeColor: "#cc0000", opacity: 100, lineWidth: 4 },
};

function resultFor(next: LineObjectView): LineEditResultView {
  return { pageIndex: 0, line: next, history: dirtyHistory };
}

const originalGetBoundingClientRect = Element.prototype.getBoundingClientRect;

beforeEach(() => {
  for (const mock of [
    createLineObjectMock,
    deleteLineObjectMock,
    pageLineObjectsMock,
    transformLineObjectMock,
    updateLineStyleMock,
  ]) {
    mock.mockReset();
  }
  pageLineObjectsMock.mockResolvedValue([]);
  transformLineObjectMock.mockImplementation(
    async (_documentId, _pageIndex, _lineId, transform) =>
      resultFor({ ...line, geometry: transform.geometry }),
  );
  updateLineStyleMock.mockImplementation(
    async (_documentId, _pageIndex, _lineId, style) =>
      resultFor({ ...line, style }),
  );
  deleteLineObjectMock.mockResolvedValue({ pageIndex: 0, history: dirtyHistory });
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
  Element.prototype.getBoundingClientRect = originalGetBoundingClientRect;
});

async function renderLayer(
  props: Partial<Parameters<typeof LineLayer>[0]> = {},
) {
  const result = render(
    <LineLayer
      documentId="doc-1"
      pageIndex={0}
      pageWidth={612}
      pageHeight={792}
      scale={1}
      canModifyContent
      {...props}
    />,
  );
  await waitFor(() => expect(pageLineObjectsMock).toHaveBeenCalled());
  return result;
}

function shaftOf(testId: string): SVGLineElement {
  const glyph = screen.getByTestId(testId);
  const shaft = glyph.querySelector("line");
  expect(shaft).not.toBeNull();
  return shaft as SVGLineElement;
}

function selectLine() {
  const hit = screen.getByTestId("line-hit-1");
  fireEvent.pointerDown(hit, { clientX: 200, clientY: 300, pointerId: 1 });
  fireEvent.pointerUp(hit, { clientX: 200, clientY: 300, pointerId: 1 });
}

function collectChangedEvents(): {
  events: CustomEvent[];
  dispose: () => void;
} {
  const events: CustomEvent[] = [];
  const listener = (event: Event) => events.push(event as CustomEvent);
  window.addEventListener(DOCUMENT_CHANGED_EVENT, listener);
  return {
    events,
    dispose: () => window.removeEventListener(DOCUMENT_CHANGED_EVENT, listener),
  };
}

describe("LineLayer", () => {
  it("draws a line whose preview matches the committed request", async () => {
    const created = {
      ...line,
      lineId: 7,
      geometry: { x1: 100, y1: 300, x2: 400, y2: 300 },
      style: { strokeColor: "#cc0000", opacity: 70, lineWidth: 5 },
    };
    createLineObjectMock.mockResolvedValue(resultFor(created));

    await renderLayer();
    fireEvent.change(screen.getByLabelText("线条颜色"), {
      target: { value: "#cc0000" },
    });
    fireEvent.change(screen.getByLabelText("线条线宽"), {
      target: { value: "5" },
    });
    fireEvent.change(screen.getByLabelText("线条透明度"), {
      target: { value: "70" },
    });
    fireEvent.click(screen.getByRole("button", { name: "直线" }));

    const surface = screen.getByTestId("line-draw-surface");
    fireEvent.pointerDown(surface, {
      clientX: 100,
      clientY: 300,
      pointerId: 1,
    });
    fireEvent.pointerMove(surface, {
      clientX: 400,
      clientY: 300,
      pointerId: 1,
    });

    const preview = screen.getByTestId("line-create-preview");
    expect(preview).toHaveAttribute("data-kind", "line");
    expect(preview).toHaveAttribute("opacity", "0.7");
    const previewShaft = preview.querySelector("line") as SVGLineElement;
    expect(previewShaft).toHaveAttribute("stroke", "#cc0000");
    expect(previewShaft).toHaveAttribute("stroke-width", "5");
    expect(previewShaft).toHaveAttribute("x1", "100");
    expect(previewShaft).toHaveAttribute("y1", "300");
    expect(previewShaft).toHaveAttribute("x2", "400");
    expect(previewShaft).toHaveAttribute("y2", "300");
    expect(screen.queryByTestId("line-create-preview-head")).not.toBeInTheDocument();

    fireEvent.pointerUp(surface, {
      clientX: 400,
      clientY: 300,
      pointerId: 1,
    });

    await waitFor(() =>
      expect(createLineObjectMock).toHaveBeenCalledWith("doc-1", {
        pageIndex: 0,
        kind: "line",
        geometry: { x1: 100, y1: 300, x2: 400, y2: 300 },
        style: { strokeColor: "#cc0000", opacity: 70, lineWidth: 5 },
      } satisfies CreateLineRequest),
    );
    expect(await screen.findByTestId("line-object-7")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "直线" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });

  it("previews an arrow head with the backend's head geometry", async () => {
    createLineObjectMock.mockResolvedValue(
      resultFor({
        ...line,
        lineId: 8,
        kind: "arrow",
        geometry: { x1: 100, y1: 300, x2: 400, y2: 300 },
      }),
    );

    await renderLayer();
    fireEvent.click(screen.getByRole("button", { name: "箭头" }));
    const surface = screen.getByTestId("line-draw-surface");
    fireEvent.pointerDown(surface, { clientX: 100, clientY: 300, pointerId: 1 });
    fireEvent.pointerMove(surface, { clientX: 400, clientY: 300, pointerId: 1 });

    const preview = screen.getByTestId("line-create-preview");
    expect(preview).toHaveAttribute("data-kind", "arrow");
    const head = screen.getByTestId("line-create-preview-head");
    const points = (head.getAttribute("points") ?? "")
      .split(" ")
      .map((pair) => pair.split(",").map(Number));
    expect(points).toHaveLength(3);
    expect(points[0][0]).toBeCloseTo(400, 6);
    expect(points[0][1]).toBeCloseTo(300, 6);
    expect(points[1][0]).toBeCloseTo(392, 6);
    expect(points[1][1]).toBeCloseTo(302.8, 6);
    expect(points[2][0]).toBeCloseTo(392, 6);
    expect(points[2][1]).toBeCloseTo(297.2, 6);

    fireEvent.pointerUp(surface, { clientX: 400, clientY: 300, pointerId: 1 });
    await waitFor(() =>
      expect(createLineObjectMock).toHaveBeenCalledWith(
        "doc-1",
        expect.objectContaining({ kind: "arrow" }),
      ),
    );
  });

  it("moves a line with one transform action and follows the result", async () => {
    pageLineObjectsMock.mockResolvedValue([line]);
    await renderLayer();

    const hit = await screen.findByTestId("line-hit-1");
    fireEvent.pointerDown(hit, { clientX: 200, clientY: 300, pointerId: 1 });
    fireEvent.pointerMove(hit, { clientX: 250, clientY: 350, pointerId: 1 });
    fireEvent.pointerUp(hit, { clientX: 250, clientY: 350, pointerId: 1 });

    await waitFor(() =>
      expect(transformLineObjectMock).toHaveBeenCalledWith("doc-1", 0, 1, {
        geometry: { x1: 150, y1: 350, x2: 350, y2: 350 },
      }),
    );
    await waitFor(() => {
      const shaft = shaftOf("line-object-1");
      expect(shaft).toHaveAttribute("x1", "150");
      expect(shaft).toHaveAttribute("y1", "350");
      expect(shaft).toHaveAttribute("x2", "350");
      expect(shaft).toHaveAttribute("y2", "350");
    });
  });

  it("moves only the dragged endpoint", async () => {
    pageLineObjectsMock.mockResolvedValue([line]);
    await renderLayer();
    await screen.findByTestId("line-hit-1");
    selectLine();

    const end = await screen.findByTestId("line-endpoint-end");
    fireEvent.pointerDown(end, { clientX: 300, clientY: 300, pointerId: 2 });
    fireEvent.pointerMove(end, { clientX: 250, clientY: 400, pointerId: 2 });
    fireEvent.pointerUp(end, { clientX: 250, clientY: 400, pointerId: 2 });

    await waitFor(() =>
      expect(transformLineObjectMock).toHaveBeenCalledWith("doc-1", 0, 1, {
        geometry: { x1: 100, y1: 300, x2: 250, y2: 400 },
      }),
    );
  });

  it("rotates the real endpoints around the midpoint", async () => {
    pageLineObjectsMock.mockResolvedValue([line]);
    await renderLayer();
    await screen.findByTestId("line-hit-1");
    selectLine();

    const handle = await screen.findByTestId("line-rotate-handle");
    // The handle sits 18pt above the midpoint (200, 300). Dragging it to the
    // right of the midpoint is a 90-degree clockwise rotation.
    fireEvent.pointerDown(handle, { clientX: 200, clientY: 282, pointerId: 3 });
    fireEvent.pointerMove(handle, { clientX: 218, clientY: 300, pointerId: 3 });
    fireEvent.pointerUp(handle, { clientX: 218, clientY: 300, pointerId: 3 });

    await waitFor(() => expect(transformLineObjectMock).toHaveBeenCalled());
    const request = transformLineObjectMock.mock.calls[0][3] as {
      geometry: { x1: number; y1: number; x2: number; y2: number };
    };
    expect(request.geometry.x1).toBeCloseTo(200, 6);
    expect(request.geometry.y1).toBeCloseTo(200, 6);
    expect(request.geometry.x2).toBeCloseTo(200, 6);
    expect(request.geometry.y2).toBeCloseTo(400, 6);
  });

  it("restyles and deletes the selected line through the property panel", async () => {
    pageLineObjectsMock.mockResolvedValue([line]);
    const restyled = {
      ...line,
      style: { strokeColor: "#00aa00", opacity: 40, lineWidth: 6 },
    };
    updateLineStyleMock.mockResolvedValue(resultFor(restyled));
    await renderLayer();
    await screen.findByTestId("line-hit-1");
    const changed = collectChangedEvents();
    selectLine();
    const panel = await screen.findByLabelText("线条属性");
    expect(panel).toHaveTextContent("直线 · 长度 200 pt · 角度 0°");
    expect(panel).toHaveTextContent("线条是页面内容，不是注释");
    fireEvent.change(screen.getByLabelText("线条描边颜色"), {
      target: { value: "#00aa00" },
    });
    fireEvent.change(screen.getByLabelText("线条描边宽度"), {
      target: { value: "6" },
    });
    fireEvent.change(screen.getByLabelText("线条描边透明度"), {
      target: { value: "40" },
    });
    fireEvent.click(screen.getByRole("button", { name: "应用样式" }));

    await waitFor(() =>
      expect(updateLineStyleMock).toHaveBeenCalledWith("doc-1", 0, 1, {
        strokeColor: "#00aa00",
        opacity: 40,
        lineWidth: 6,
      }),
    );
    expect(
      changed.events.some(
        (event) =>
          (event.detail as { source?: string }).source === "line",
      ),
    ).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "删除线条" }));
    await waitFor(() => expect(deleteLineObjectMock).toHaveBeenCalledWith("doc-1", 0, 1));
    await waitFor(() =>
      expect(screen.queryByTestId("line-object-1")).not.toBeInTheDocument(),
    );
    expect(screen.queryByLabelText("线条属性")).not.toBeInTheDocument();
    changed.dispose();
  });

  it("does not create an object for a click or a sliver", async () => {
    await renderLayer();
    fireEvent.click(screen.getByRole("button", { name: "直线" }));
    const surface = screen.getByTestId("line-draw-surface");
    fireEvent.pointerDown(surface, { clientX: 100, clientY: 300, pointerId: 1 });
    fireEvent.pointerUp(surface, { clientX: 101, clientY: 300, pointerId: 1 });
    await waitFor(() =>
      expect(screen.queryByTestId("line-create-preview")).not.toBeInTheDocument(),
    );
    expect(createLineObjectMock).not.toHaveBeenCalled();
  });

  it("keeps the layer read-only without content permission", async () => {
    pageLineObjectsMock.mockResolvedValue([line]);
    await renderLayer({ canModifyContent: false });

    const hit = await screen.findByTestId("line-hit-1");
    expect(
      screen.queryByRole("toolbar", { name: "直线与箭头工具" }),
    ).not.toBeInTheDocument();
    fireEvent.pointerDown(hit, { clientX: 200, clientY: 300, pointerId: 1 });
    fireEvent.pointerMove(hit, { clientX: 260, clientY: 360, pointerId: 1 });
    fireEvent.pointerUp(hit, { clientX: 260, clientY: 360, pointerId: 1 });
    expect(transformLineObjectMock).not.toHaveBeenCalled();
    expect(screen.queryByLabelText("线条属性")).not.toBeInTheDocument();
  });

  it("refetches its objects when undo or another tool changes the page", async () => {
    pageLineObjectsMock.mockResolvedValueOnce([line]).mockResolvedValue([]);
    await renderLayer();
    expect(await screen.findByTestId("line-object-1")).toBeInTheDocument();

    notifyDocumentChanged({ documentId: "doc-1", pageIndex: 0, source: "text" });

    await waitFor(() =>
      expect(screen.queryByTestId("line-object-1")).not.toBeInTheDocument(),
    );
  });

  it("shows a Chinese error when the backend rejects the line", async () => {
    createLineObjectMock.mockRejectedValue({
      code: "permission_denied",
      message: "this document does not allow changing its content",
    });
    await renderLayer();
    fireEvent.click(screen.getByRole("button", { name: "箭头" }));
    const surface = screen.getByTestId("line-draw-surface");
    fireEvent.pointerDown(surface, { clientX: 100, clientY: 300, pointerId: 1 });
    fireEvent.pointerMove(surface, { clientX: 400, clientY: 300, pointerId: 1 });
    fireEvent.pointerUp(surface, { clientX: 400, clientY: 300, pointerId: 1 });

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "文档所有者限制了编辑权限。",
    );
    expect(screen.queryByTestId("line-object-7")).not.toBeInTheDocument();
  });
});
