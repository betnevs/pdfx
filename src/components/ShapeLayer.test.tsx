import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { AnnotationView, EditHistoryView, ShapeStyle } from "../lib/backend";
import { DOCUMENT_CHANGED_EVENT } from "../lib/documentEvents";
import { ShapeLayer } from "./ShapeLayer";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

const dirtyHistory: EditHistoryView = {
  canUndo: true,
  canRedo: false,
  revision: 1,
  savedRevision: 0,
  dirty: true,
};

function style(overrides: Partial<ShapeStyle> = {}): ShapeStyle {
  return {
    fillColor: "#3366cc",
    strokeColor: "#000000",
    opacity: 100,
    lineWidth: 2,
    ...overrides,
  };
}

const createdRectangle: AnnotationView = {
  name: "pdfx-7",
  pageIndex: 2,
  kind: "square",
  author: "测试作者",
  createdAt: "D:20261028123456Z",
  color: "#000000",
  opacity: 100,
  rects: [],
  shape: {
    kind: "square",
    geometry: { x: 100, y: 200, width: 200, height: 60, rotation: 0 },
    style: style(),
  },
};

const selectedRectangle: AnnotationView = {
  name: "pdfx-8",
  pageIndex: 2,
  kind: "square",
  author: "测试作者",
  createdAt: "D:20261028123456Z",
  color: "#000000",
  opacity: 80,
  rects: [],
  shape: {
    kind: "square",
    geometry: { x: 100, y: 100, width: 200, height: 100, rotation: 0 },
    style: style({ opacity: 80, lineWidth: 3 }),
  },
};

function mockCommands(
  overrides: Record<string, (args: Record<string, unknown>) => unknown> = {},
) {
  const handlers: Record<string, (args: Record<string, unknown>) => unknown> = {
    create_shape_annotation: () => ({
      annotation: createdRectangle,
      pageIndex: 2,
      history: dirtyHistory,
    }),
    update_shape_geometry: () => ({
      annotation: selectedRectangle,
      pageIndex: 2,
      history: dirtyHistory,
    }),
    update_shape_style: () => ({
      annotation: selectedRectangle,
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

function renderLayer(props: Partial<Parameters<typeof ShapeLayer>[0]> = {}) {
  const onSelect = vi.fn();
  const result = render(
    <ShapeLayer
      documentId="doc-1"
      pageIndex={2}
      pageWidth={612}
      pageHeight={792}
      scale={1}
      canAnnotate
      annotations={[]}
      selectedAnnotation={null}
      onSelectAnnotation={onSelect}
      {...props}
    />,
  );
  return { ...result, onSelect };
}

function events(): CustomEvent[] {
  const received: CustomEvent[] = [];
  const listener = (event: Event) => received.push(event as CustomEvent);
  window.addEventListener(DOCUMENT_CHANGED_EVENT, listener);
  return received;
}

describe("ShapeLayer toolbar", () => {
  it("hides shape tools when annotation changes are not permitted", () => {
    renderLayer({ canAnnotate: false });
    expect(screen.queryByRole("button", { name: "矩形" })).toBeNull();
    expect(screen.queryByRole("button", { name: "椭圆" })).toBeNull();
  });

  it("hides shape tools on a read-only document", () => {
    renderLayer({ readOnly: true });
    expect(screen.queryByRole("button", { name: "矩形" })).toBeNull();
  });

  it("cancels the active tool with Escape", () => {
    renderLayer();
    fireEvent.click(screen.getByRole("button", { name: "矩形" }));
    expect(screen.getByTestId("shape-draw-surface")).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByTestId("shape-draw-surface")).toBeNull();
    expect(screen.getByRole("button", { name: "矩形" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });
});

describe("ShapeLayer drawing", () => {
  it("draws a rectangle with the configured fill, stroke, opacity and line width", async () => {
    const calls: Record<string, unknown>[] = [];
    mockCommands({
      create_shape_annotation: (args) => {
        calls.push(args);
        return {
          annotation: createdRectangle,
          pageIndex: 2,
          history: dirtyHistory,
        };
      },
    });
    const received = events();
    const { onSelect } = renderLayer();

    fireEvent.click(screen.getByRole("button", { name: "矩形" }));
    fireEvent.change(screen.getByLabelText("填充颜色"), {
      target: { value: "#ff0000" },
    });
    fireEvent.change(screen.getByLabelText("描边颜色"), {
      target: { value: "#00ff00" },
    });
    fireEvent.change(screen.getByLabelText("形状透明度"), {
      target: { value: "70" },
    });
    fireEvent.change(screen.getByLabelText("线宽"), {
      target: { value: "5" },
    });

    const surface = screen.getByTestId("shape-draw-surface");
    fireEvent.pointerDown(surface, {
      clientX: 100,
      clientY: 200,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerMove(surface, {
      clientX: 300,
      clientY: 260,
      pointerId: 1,
    });

    const preview = screen.getByTestId("shape-create-preview");
    expect(preview).toHaveAttribute("data-kind", "square");
    expect(preview).toHaveAttribute("x", "100");
    expect(preview).toHaveAttribute("y", "200");
    expect(preview).toHaveAttribute("width", "200");
    expect(preview).toHaveAttribute("height", "60");
    expect(preview).toHaveAttribute("fill", "#ff0000");
    expect(preview).toHaveAttribute("stroke", "#00ff00");
    expect(preview).toHaveAttribute("stroke-width", "5");
    expect(preview).toHaveAttribute("opacity", "0.7");

    fireEvent.pointerUp(surface, {
      clientX: 300,
      clientY: 260,
      pointerId: 1,
    });

    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]).toMatchObject({
      documentId: "doc-1",
      request: {
        pageIndex: 2,
        kind: "square",
        geometry: { x: 100, y: 200, width: 200, height: 60, rotation: 0 },
        style: {
          fillColor: "#ff0000",
          strokeColor: "#00ff00",
          opacity: 70,
          lineWidth: 5,
        },
      },
    });
    expect(onSelect).toHaveBeenCalledWith(createdRectangle);
    expect(received.at(-1)?.detail).toEqual({
      documentId: "doc-1",
      pageIndex: 2,
      source: "shape",
    });
    await waitFor(() =>
      expect(screen.queryByTestId("shape-create-preview")).toBeNull(),
    );
    expect(screen.queryByTestId("shape-draw-surface")).toBeNull();
  });

  it("creates an ellipse with no fill", async () => {
    const calls: Record<string, unknown>[] = [];
    mockCommands({
      create_shape_annotation: (args) => {
        calls.push(args);
        return {
          annotation: { ...createdRectangle, kind: "circle" },
          pageIndex: 2,
          history: dirtyHistory,
        };
      },
    });
    renderLayer();

    fireEvent.click(screen.getByRole("button", { name: "椭圆" }));
    fireEvent.click(screen.getByLabelText("填充"));

    const surface = screen.getByTestId("shape-draw-surface");
    fireEvent.pointerDown(surface, {
      clientX: 200,
      clientY: 300,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerMove(surface, {
      clientX: 400,
      clientY: 360,
      pointerId: 1,
    });
    expect(screen.getByTestId("shape-create-preview")).toHaveAttribute(
      "data-kind",
      "circle",
    );
    fireEvent.pointerUp(surface, {
      clientX: 400,
      clientY: 360,
      pointerId: 1,
    });

    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]).toMatchObject({
      request: {
        kind: "circle",
        style: { fillColor: null, strokeColor: "#000000", lineWidth: 2 },
      },
    });
  });

  it("disables drawing and the line width control with no fill and no stroke", () => {
    renderLayer();
    fireEvent.click(screen.getByLabelText("填充"));
    fireEvent.click(screen.getByLabelText("描边"));
    expect(screen.getByRole("button", { name: "矩形" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "椭圆" })).toBeDisabled();
    expect(screen.getByLabelText("线宽")).toBeDisabled();
    expect(
      screen.getByText("至少需要填充或描边中的一项"),
    ).toBeInTheDocument();
  });

  it("ignores a click that is not a drag", async () => {
    mockCommands();
    renderLayer();
    fireEvent.click(screen.getByRole("button", { name: "矩形" }));
    const surface = screen.getByTestId("shape-draw-surface");
    fireEvent.pointerDown(surface, {
      clientX: 100,
      clientY: 200,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerUp(surface, {
      clientX: 100,
      clientY: 200,
      pointerId: 1,
    });
    await waitFor(() =>
      expect(screen.queryByTestId("shape-draw-surface")).not.toBeNull(),
    );
    expect(invokeMock).not.toHaveBeenCalled();
    expect(screen.queryByTestId("shape-create-preview")).toBeNull();
  });
});

describe("ShapeLayer selection and transforms", () => {
  it("draws a rotated selection outline and moves the selected shape", async () => {
    const calls: Record<string, unknown>[] = [];
    mockCommands({
      update_shape_geometry: (args) => {
        calls.push(args);
        return {
          annotation: selectedRectangle,
          pageIndex: 2,
          history: dirtyHistory,
        };
      },
    });
    const received = events();
    const { onSelect } = renderLayer({
      annotations: [selectedRectangle],
      selectedAnnotation: selectedRectangle,
    });

    const outline = screen.getByTestId("shape-selection-outline");
    expect(outline).toHaveAttribute("transform", "rotate(0 200 150)");
    expect(outline).toHaveAttribute("stroke-dasharray");

    const hit = screen.getByTestId("shape-hit-pdfx-8");
    fireEvent.pointerDown(hit, {
      clientX: 150,
      clientY: 150,
      button: 0,
      pointerId: 1,
    });
    expect(onSelect).toHaveBeenCalledWith(selectedRectangle);
    fireEvent.pointerMove(hit, {
      clientX: 170,
      clientY: 160,
      pointerId: 1,
    });
    expect(screen.getByTestId("shape-transform-preview")).toBeInTheDocument();
    fireEvent.pointerUp(hit, {
      clientX: 170,
      clientY: 160,
      pointerId: 1,
    });

    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]).toMatchObject({
      documentId: "doc-1",
      name: "pdfx-8",
      geometry: { x: 120, y: 110, width: 200, height: 100, rotation: 0 },
    });
    expect(received.at(-1)?.detail).toEqual({
      documentId: "doc-1",
      pageIndex: 2,
      source: "shape",
    });
  });

  it("resizes a rotated shape along its own axes", async () => {
    const rotated: AnnotationView = {
      ...selectedRectangle,
      shape: {
        kind: "square",
        geometry: { x: 100, y: 100, width: 100, height: 60, rotation: 90 },
        style: selectedRectangle.shape!.style,
      },
    };
    const calls: Record<string, unknown>[] = [];
    mockCommands({
      update_shape_geometry: (args) => {
        calls.push(args);
        return { annotation: rotated, pageIndex: 2, history: dirtyHistory };
      },
    });
    renderLayer({ annotations: [rotated], selectedAnnotation: rotated });

    expect(screen.getByTestId("shape-selection-outline")).toHaveAttribute(
      "transform",
      "rotate(90 150 130)",
    );
    // Local bottom-right (50, 30) rotated 90 degrees around (150, 130).
    const handle = screen.getByTestId("shape-resize-handle");
    fireEvent.pointerDown(handle, {
      clientX: 120,
      clientY: 180,
      button: 0,
      pointerId: 1,
    });
    // +20 along the shape's local x axis is +20 in display y at 90 degrees.
    fireEvent.pointerMove(handle, {
      clientX: 120,
      clientY: 200,
      pointerId: 1,
    });
    fireEvent.pointerUp(handle, {
      clientX: 120,
      clientY: 200,
      pointerId: 1,
    });

    await waitFor(() => expect(calls).toHaveLength(1));
    const geometry = (calls[0] as { geometry: Record<string, number> }).geometry;
    expect(geometry.width).toBeCloseTo(120, 4);
    expect(geometry.height).toBeCloseTo(60, 4);
    expect(geometry.rotation).toBe(90);
    // The fixed top-left corner sits at (180, 80); growing width by 20 moves
    // the centre 10pt along the rotated x axis (display y at 90 degrees).
    expect(geometry.x + geometry.width / 2).toBeCloseTo(150, 4);
    expect(geometry.y + geometry.height / 2).toBeCloseTo(140, 4);
  });

  it("rotates with the handle and sends the real rotation", async () => {
    const calls: Record<string, unknown>[] = [];
    mockCommands({
      update_shape_geometry: (args) => {
        calls.push(args);
        return {
          annotation: selectedRectangle,
          pageIndex: 2,
          history: dirtyHistory,
        };
      },
    });
    renderLayer({
      annotations: [selectedRectangle],
      selectedAnnotation: selectedRectangle,
    });

    const handle = screen.getByTestId("shape-rotate-handle");
    fireEvent.pointerDown(handle, {
      clientX: 200,
      clientY: 82,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerMove(handle, {
      clientX: 268,
      clientY: 150,
      pointerId: 1,
    });
    fireEvent.pointerUp(handle, {
      clientX: 268,
      clientY: 150,
      pointerId: 1,
    });

    await waitFor(() => expect(calls).toHaveLength(1));
    const geometry = (calls[0] as { geometry: Record<string, number> }).geometry;
    expect(geometry.rotation).toBeCloseTo(90, 4);
    expect(geometry.width).toBe(200);
    expect(geometry.height).toBe(100);
  });

  it("selects without moving when a shape is clicked without dragging", async () => {
    mockCommands();
    const { onSelect } = renderLayer({
      annotations: [selectedRectangle],
      selectedAnnotation: null,
    });
    const hit = screen.getByTestId("shape-hit-pdfx-8");
    fireEvent.pointerDown(hit, {
      clientX: 150,
      clientY: 150,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerUp(hit, {
      clientX: 150,
      clientY: 150,
      pointerId: 1,
    });
    expect(onSelect).toHaveBeenCalledWith(selectedRectangle);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("does not move shapes without annotation permission", () => {
    mockCommands();
    const { onSelect } = renderLayer({
      canAnnotate: false,
      annotations: [selectedRectangle],
      selectedAnnotation: selectedRectangle,
    });
    expect(screen.queryByTestId("shape-resize-handle")).toBeNull();
    expect(screen.queryByTestId("shape-rotate-handle")).toBeNull();
    const hit = screen.getByTestId("shape-hit-pdfx-8");
    fireEvent.pointerDown(hit, {
      clientX: 150,
      clientY: 150,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerMove(hit, {
      clientX: 200,
      clientY: 200,
      pointerId: 1,
    });
    fireEvent.pointerUp(hit, {
      clientX: 200,
      clientY: 200,
      pointerId: 1,
    });
    expect(onSelect).toHaveBeenCalledWith(selectedRectangle);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("shows a backend error without dropping the shape", async () => {
    mockCommands({
      update_shape_geometry: () => {
        throw new Error("文档已关闭");
      },
    });
    renderLayer({
      annotations: [selectedRectangle],
      selectedAnnotation: selectedRectangle,
    });
    const hit = screen.getByTestId("shape-hit-pdfx-8");
    fireEvent.pointerDown(hit, {
      clientX: 150,
      clientY: 150,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerMove(hit, {
      clientX: 170,
      clientY: 160,
      pointerId: 1,
    });
    fireEvent.pointerUp(hit, {
      clientX: 170,
      clientY: 160,
      pointerId: 1,
    });
    expect(await screen.findByRole("alert")).toHaveTextContent("文档已关闭");
    expect(screen.getByTestId("shape-selection-outline")).toBeInTheDocument();
  });
});
