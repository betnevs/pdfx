import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  createFreeTextAnnotation,
  transformFreeTextAnnotation,
  updateFreeText,
  type AnnotationView,
  type EditHistoryView,
} from "../lib/backend";
import { DOCUMENT_CHANGED_EVENT } from "../lib/documentEvents";
import { FreeTextLayer, type FreeTextLayerProps } from "./FreeTextLayer";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../lib/backend", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/backend")>();
  return {
    ...actual,
    createFreeTextAnnotation: vi.fn(),
    updateFreeText: vi.fn(),
    transformFreeTextAnnotation: vi.fn(),
  };
});

const createMock = vi.mocked(createFreeTextAnnotation);
const updateMock = vi.mocked(updateFreeText);
const transformMock = vi.mocked(transformFreeTextAnnotation);

const history: EditHistoryView = {
  canUndo: true,
  canRedo: false,
  revision: 1,
  savedRevision: 0,
  dirty: true,
};

const plain: AnnotationView = {
  name: "ft-1",
  pageIndex: 0,
  kind: "freetext",
  author: "测试作者",
  createdAt: "D:20261028123456Z",
  color: "#111827",
  opacity: 100,
  lineWidth: null,
  text: "Hello",
  strokes: [],
  rects: [{ x: 100, y: 200, width: 200, height: 60 }],
  shape: null,
  stamp: null,
  freeText: { kind: "freetext", fontSize: 14, align: "left" },
};

const callout: AnnotationView = {
  ...plain,
  name: "ft-2",
  kind: "callout",
  text: "标注框",
  rects: [{ x: 320, y: 100, width: 160, height: 80 }],
  freeText: { kind: "callout", fontSize: 16, align: "center" },
};

function renderLayer(overrides: Partial<FreeTextLayerProps> = {}) {
  const onSelect = vi.fn();
  const result = render(
    <FreeTextLayer
      documentId="doc-1"
      pageIndex={0}
      pageWidth={612}
      pageHeight={792}
      canAnnotate
      annotations={[]}
      selectedAnnotation={null}
      onSelectAnnotation={onSelect}
      {...overrides}
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

function draw(
  from: { x: number; y: number },
  to: { x: number; y: number },
) {
  const surface = screen.getByTestId("free-text-draw-surface");
  fireEvent.pointerDown(surface, {
    clientX: from.x,
    clientY: from.y,
    button: 0,
  });
  fireEvent.pointerMove(surface, { clientX: to.x, clientY: to.y });
  fireEvent.pointerUp(surface, { clientX: to.x, clientY: to.y });
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
  createMock.mockResolvedValue({
    annotation: { ...plain, name: "ft-new", text: "", rects: [{ x: 100, y: 200, width: 200, height: 60 }] },
    pageIndex: 0,
    history,
  });
  updateMock.mockResolvedValue({
    annotation: { ...plain, text: "新文本" },
    pageIndex: 0,
    history,
  });
  transformMock.mockResolvedValue({
    annotation: { ...plain, rects: [{ x: 150, y: 230, width: 200, height: 60 }] },
    pageIndex: 0,
    history,
  });
});

afterEach(() => {
  Element.prototype.getBoundingClientRect = originalGetBoundingClientRect;
  createMock.mockReset();
  updateMock.mockReset();
  transformMock.mockReset();
});

describe("FreeTextLayer permissions", () => {
  it("hides the tools and hit areas when annotations cannot be changed", () => {
    const { rerender } = renderLayer({
      canAnnotate: false,
      annotations: [plain],
    });
    expect(
      screen.queryByRole("toolbar", { name: "自由文本工具" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByTestId("free-text-hit-ft-1")).not.toBeInTheDocument();
    expect(screen.queryByTestId("free-text-resize-handle")).not.toBeInTheDocument();

    rerender(
      <FreeTextLayer
        documentId="doc-1"
        pageIndex={0}
        pageWidth={612}
        pageHeight={792}
        canAnnotate
        readOnly
        annotations={[plain]}
        selectedAnnotation={plain}
      />,
    );
    expect(
      screen.queryByRole("toolbar", { name: "自由文本工具" }),
    ).not.toBeInTheDocument();
  });
});

describe("FreeTextLayer creation", () => {
  it("places a free text box with the toolbar's defaults", async () => {
    const received = events();
    const { onSelect } = renderLayer();
    fireEvent.click(screen.getByRole("button", { name: "自由文本" }));
    draw({ x: 100, y: 200 }, { x: 300, y: 260 });
    await waitFor(() => expect(createMock).toHaveBeenCalledTimes(1));
    expect(createMock).toHaveBeenCalledWith("doc-1", {
      pageIndex: 0,
      kind: "freetext",
      rect: { x: 100, y: 200, width: 200, height: 60 },
      color: "#111827",
      opacity: 100,
      text: "",
      fontSize: 12,
      align: "left",
    });
    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ name: "ft-new" }),
    );
    expect(received).toHaveLength(1);
    expect(received[0].detail).toEqual({
      documentId: "doc-1",
      pageIndex: 0,
      source: "annotation",
    });
  });

  it("places a callout box and honours size, colour, alignment and opacity", async () => {
    renderLayer();
    fireEvent.click(screen.getByRole("button", { name: "标注框" }));
    fireEvent.change(screen.getByLabelText("文本字号"), {
      target: { value: "20" },
    });
    fireEvent.change(screen.getByLabelText("文本颜色"), {
      target: { value: "#cc0000" },
    });
    fireEvent.change(screen.getByLabelText("文本对齐"), {
      target: { value: "right" },
    });
    fireEvent.change(screen.getByLabelText("文本透明度"), {
      target: { value: "50" },
    });
    draw({ x: 320, y: 100 }, { x: 480, y: 180 });
    await waitFor(() => expect(createMock).toHaveBeenCalledTimes(1));
    expect(createMock).toHaveBeenCalledWith("doc-1", {
      pageIndex: 0,
      kind: "callout",
      rect: { x: 320, y: 100, width: 160, height: 80 },
      color: "#cc0000",
      opacity: 50,
      text: "",
      fontSize: 20,
      align: "right",
    });
  });

  it("rejects a sliver drag without touching the document", async () => {
    renderLayer();
    fireEvent.click(screen.getByRole("button", { name: "自由文本" }));
    draw({ x: 100, y: 100 }, { x: 105, y: 104 });
    await waitFor(() =>
      expect(screen.getByText("文本框至少需要 12 × 12 pt")).toBeInTheDocument(),
    );
    expect(createMock).not.toHaveBeenCalled();
  });

  it("reports the command error when placement is refused", async () => {
    createMock.mockRejectedValue({
      code: "permission_denied",
      message: "文档权限不允许修改批注",
    });
    renderLayer();
    fireEvent.click(screen.getByRole("button", { name: "自由文本" }));
    draw({ x: 100, y: 200 }, { x: 300, y: 260 });
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(
        "文档权限不允许修改批注",
      ),
    );
  });
});

describe("FreeTextLayer editing", () => {
  it("types and saves text through update_free_text", async () => {
    const received = events();
    const { onSelect } = renderLayer({
      annotations: [plain],
      selectedAnnotation: plain,
    });
    fireEvent.click(screen.getByRole("button", { name: "编辑文本" }));
    const editor = screen.getByLabelText("自由文本编辑器");
    expect(editor).toHaveValue("Hello");
    fireEvent.change(editor, { target: { value: "新文本" } });
    fireEvent.click(screen.getByRole("button", { name: "保存文本" }));
    await waitFor(() => expect(updateMock).toHaveBeenCalledTimes(1));
    expect(updateMock).toHaveBeenCalledWith("doc-1", {
      name: "ft-1",
      text: "新文本",
      fontSize: 14,
      align: "left",
      color: "#111827",
      opacity: 100,
    });
    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ name: "ft-1", text: "新文本" }),
    );
    expect(received[0].detail.source).toBe("annotation");
    await waitFor(() =>
      expect(screen.queryByLabelText("自由文本编辑器")).not.toBeInTheDocument(),
    );
  });

  it("opens the editor by double-clicking the box", () => {
    renderLayer({ annotations: [plain], selectedAnnotation: plain });
    fireEvent.doubleClick(screen.getByTestId("free-text-hit-ft-1"));
    expect(screen.getByLabelText("自由文本编辑器")).toHaveValue("Hello");
  });

  it("cancels editing without a command", () => {
    renderLayer({ annotations: [plain], selectedAnnotation: plain });
    fireEvent.click(screen.getByRole("button", { name: "编辑文本" }));
    fireEvent.change(screen.getByLabelText("自由文本编辑器"), {
      target: { value: "不要保存" },
    });
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(screen.queryByLabelText("自由文本编辑器")).not.toBeInTheDocument();
    expect(updateMock).not.toHaveBeenCalled();
  });

  it("keeps the editor open and shows the error when saving is refused", async () => {
    updateMock.mockRejectedValue({
      code: "permission_denied",
      message: "文档权限不允许修改批注",
    });
    renderLayer({ annotations: [plain], selectedAnnotation: plain });
    fireEvent.click(screen.getByRole("button", { name: "编辑文本" }));
    fireEvent.change(screen.getByLabelText("自由文本编辑器"), {
      target: { value: "新文本" },
    });
    fireEvent.click(screen.getByRole("button", { name: "保存文本" }));
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(
        "文档权限不允许修改批注",
      ),
    );
    expect(screen.getByLabelText("自由文本编辑器")).toBeInTheDocument();
  });
});

describe("FreeTextLayer transforms", () => {
  it("moves a box as one transform action", async () => {
    const received = events();
    const { onSelect } = renderLayer({
      annotations: [plain],
      selectedAnnotation: plain,
    });
    const hit = screen.getByTestId("free-text-hit-ft-1");
    fireEvent.pointerDown(hit, { clientX: 150, clientY: 220, button: 0 });
    fireEvent.pointerMove(hit, { clientX: 200, clientY: 250 });
    fireEvent.pointerUp(hit, { clientX: 200, clientY: 250 });
    await waitFor(() => expect(transformMock).toHaveBeenCalledTimes(1));
    const [, name, rect] = transformMock.mock.calls[0];
    expect(name).toBe("ft-1");
    expect(rect.x).toBeCloseTo(150, 6);
    expect(rect.y).toBeCloseTo(230, 6);
    expect(rect.width).toBe(200);
    expect(rect.height).toBe(60);
    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ name: "ft-1" }),
    );
    expect(received[0].detail.source).toBe("annotation");
  });

  it("selects on a click without committing a transform", async () => {
    const { onSelect } = renderLayer({
      annotations: [plain],
      selectedAnnotation: plain,
    });
    const hit = screen.getByTestId("free-text-hit-ft-1");
    fireEvent.pointerDown(hit, { clientX: 150, clientY: 220, button: 0 });
    fireEvent.pointerUp(hit, { clientX: 150, clientY: 220 });
    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ name: "ft-1" }),
    );
    expect(transformMock).not.toHaveBeenCalled();
  });

  it("resizes from the corner handle", async () => {
    renderLayer({ annotations: [plain], selectedAnnotation: plain });
    const handle = screen.getByTestId("free-text-resize-handle");
    fireEvent.pointerDown(handle, { clientX: 300, clientY: 260, button: 0 });
    fireEvent.pointerMove(handle, { clientX: 360, clientY: 300 });
    fireEvent.pointerUp(handle, { clientX: 360, clientY: 300 });
    await waitFor(() => expect(transformMock).toHaveBeenCalledTimes(1));
    expect(transformMock).toHaveBeenCalledWith("doc-1", "ft-1", {
      x: 100,
      y: 200,
      width: 260,
      height: 100,
    });
  });

  it("reports a refused transform and leaves the box unchanged", async () => {
    transformMock.mockRejectedValue({
      code: "permission_denied",
      message: "文档权限不允许修改批注",
    });
    renderLayer({ annotations: [plain], selectedAnnotation: plain });
    const hit = screen.getByTestId("free-text-hit-ft-1");
    fireEvent.pointerDown(hit, { clientX: 150, clientY: 220, button: 0 });
    fireEvent.pointerMove(hit, { clientX: 200, clientY: 250 });
    fireEvent.pointerUp(hit, { clientX: 200, clientY: 250 });
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(
        "文档权限不允许修改批注",
      ),
    );
  });

  it("only renders annotations belonging to this page", () => {
    renderLayer({
      pageIndex: 1,
      annotations: [
        { ...plain, name: "other-page", pageIndex: 0 },
        { ...callout, name: "this-page", pageIndex: 1 },
      ],
      selectedAnnotation: null,
    });
    expect(screen.queryByTestId("free-text-hit-other-page")).not.toBeInTheDocument();
    expect(screen.getByTestId("free-text-hit-this-page")).toBeInTheDocument();
  });
});
