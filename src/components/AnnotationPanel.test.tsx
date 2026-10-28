import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { AnnotationView, EditHistoryView } from "../lib/backend";
import { AnnotationPanel } from "./AnnotationPanel";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const invokeMock = vi.mocked(invoke);

const idleHistory: EditHistoryView = {
  canUndo: false,
  canRedo: false,
  revision: 0,
  savedRevision: 0,
  dirty: false,
};

const highlight: AnnotationView = {
  name: "a1",
  pageIndex: 0,
  kind: "highlight",
  author: "测试作者",
  createdAt: "D:20261028123456Z",
  color: "#ffd400",
  opacity: 50,
  lineWidth: null,
  text: null,
  strokes: [],
  rects: [{ x: 100, y: 100, width: 120, height: 20 }],
  shape: null,
};

const underline: AnnotationView = {
  name: "a2",
  pageIndex: 1,
  kind: "underline",
  author: null,
  createdAt: null,
  color: "#00aa00",
  opacity: 100,
  lineWidth: null,
  text: null,
  strokes: [],
  rects: [{ x: 80, y: 200, width: 90, height: 18 }],
  shape: null,
};

const rectangle: AnnotationView = {
  name: "s1",
  pageIndex: 0,
  kind: "square",
  author: "形状作者",
  createdAt: "D:20261028123456Z",
  color: "#000000",
  opacity: 80,
  lineWidth: null,
  text: null,
  strokes: [],
  rects: [],
  shape: {
    kind: "square",
    geometry: { x: 100, y: 100, width: 200, height: 100, rotation: 30 },
    style: {
      fillColor: "#3366cc",
      strokeColor: "#000000",
      opacity: 80,
      lineWidth: 3,
    },
  },
};

const note: AnnotationView = {
  name: "note-1",
  pageIndex: 0,
  kind: "note",
  author: "便签作者",
  createdAt: "D:20261028130000Z",
  color: "#f9a825",
  opacity: 100,
  lineWidth: null,
  text: "稍后回来",
  strokes: [],
  rects: [{ x: 200, y: 220, width: 28, height: 28 }],
  shape: null,
};

const ink: AnnotationView = {
  name: "ink-1",
  pageIndex: 0,
  kind: "ink",
  author: "手绘作者",
  createdAt: "D:20261028131000Z",
  color: "#c62828",
  opacity: 80,
  lineWidth: 6,
  text: null,
  strokes: [
    [
      { x: 100, y: 300 },
      { x: 140, y: 340 },
    ],
    [
      { x: 200, y: 300 },
      { x: 240, y: 300 },
    ],
  ],
  rects: [{ x: 97, y: 297, width: 146, height: 46 }],
  shape: null,
};

function renderPanel(
  overrides: Partial<Parameters<typeof AnnotationPanel>[0]> = {},
) {
  const onSelect = vi.fn();
  const onChanged = vi.fn();
  render(
    <AnnotationPanel
      documentId="doc-1"
      annotations={[underline, highlight]}
      selectedName={null}
      canAnnotate
      readOnly={false}
      onSelect={onSelect}
      onChanged={onChanged}
      {...overrides}
    />,
  );
  return { onSelect, onChanged };
}

beforeEach(() => {
  invokeMock.mockImplementation(async (command) => {
    if (command === "update_annotation_style") {
      return {
        annotation: { ...highlight, color: "#00ff00", opacity: 70 },
        pageIndex: 0,
        history: idleHistory,
      };
    }
    if (command === "delete_annotation") {
      return { annotation: null, pageIndex: 0, history: idleHistory };
    }
    if (command === "update_note_text") {
      return {
        annotation: { ...note, text: "修订后的内容" },
        pageIndex: 0,
        history: idleHistory,
      };
    }
    throw new Error(`unexpected command: ${command}`);
  });
});

afterEach(() => {
  invokeMock.mockReset();
});

describe("AnnotationPanel", () => {
  it("groups annotations by page and shows kind, author and creation time", () => {
    renderPanel();

    const pageOne = screen.getByRole("heading", { name: "第 1 页" });
    const pageTwo = screen.getByRole("heading", { name: "第 2 页" });
    expect(pageOne).toBeInTheDocument();
    expect(pageTwo).toBeInTheDocument();
    expect(pageOne.compareDocumentPosition(pageTwo)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );

    expect(
      screen.getByRole("button", { name: "高亮，第 1 页，测试作者" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "下划线，第 2 页，未知作者" }),
    ).toBeInTheDocument();
    expect(screen.getByText("2026-10-28 12:34 UTC")).toBeInTheDocument();
  });

  it("selects an annotation and applies a colour and opacity change", async () => {
    const { onSelect, onChanged } = renderPanel({ selectedName: "a1" });

    fireEvent.click(
      screen.getByRole("button", { name: "高亮，第 1 页，测试作者" }),
    );
    expect(onSelect).toHaveBeenCalledWith(highlight);

    const color = screen.getByLabelText("注释颜色");
    const opacity = screen.getByLabelText("注释透明度");
    expect(color).toHaveValue("#ffd400");
    expect(opacity).toHaveValue("50");

    fireEvent.change(color, { target: { value: "#00ff00" } });
    fireEvent.change(opacity, { target: { value: "70" } });
    fireEvent.click(screen.getByRole("button", { name: "应用样式" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("update_annotation_style", {
        documentId: "doc-1",
        name: "a1",
        color: "#00ff00",
        opacity: 70,
        lineWidth: null,
      });
    });
    expect(onChanged).toHaveBeenCalled();
  });

  it("deletes the selected annotation and clears the selection", async () => {
    const { onSelect, onChanged } = renderPanel({ selectedName: "a1" });

    fireEvent.click(screen.getByRole("button", { name: "删除注释" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("delete_annotation", {
        documentId: "doc-1",
        name: "a1",
      });
    });
    expect(onSelect).toHaveBeenCalledWith(null);
    expect(onChanged).toHaveBeenCalled();
  });

  it("refuses changes when annotations are restricted and explains why", () => {
    renderPanel({ selectedName: "a1", canAnnotate: false });

    expect(screen.getByText("此文档不允许添加或修改注释")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "删除注释" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByLabelText("注释颜色")).not.toBeInTheDocument();
  });

  it("shows an empty state when the document has no annotations", () => {
    renderPanel({ annotations: [] });

    expect(screen.getByText("还没有注释")).toBeInTheDocument();
    expect(
      screen.getByText(
        "选择页面文字即可添加高亮、下划线或删除线；使用页面上的形状工具绘制矩形或椭圆，也可以放置便签或手绘。",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { name: "第 1 页" }),
    ).not.toBeInTheDocument();
  });

  it("reports a failed deletion without claiming success", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "delete_annotation") {
        throw { code: "permission_denied", message: "不允许删除注释" };
      }
      throw new Error(`unexpected command: ${command}`);
    });
    const { onSelect } = renderPanel({ selectedName: "a1" });

    fireEvent.click(screen.getByRole("button", { name: "删除注释" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("不允许删除注释");
    expect(onSelect).not.toHaveBeenCalledWith(null);
  });

  it("labels notes and ink and previews their content", () => {
    renderPanel({ annotations: [note, ink] });

    expect(
      screen.getByRole("button", { name: "便签，第 1 页，便签作者" }),
    ).toBeInTheDocument();
    expect(screen.getByText("稍后回来")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "手绘，第 1 页，手绘作者" }),
    ).toBeInTheDocument();
    expect(screen.getByText("2 笔 · 6 pt")).toBeInTheDocument();
  });

  it("saves edited note text through the unified history", async () => {
    const { onChanged } = renderPanel({
      annotations: [note],
      selectedName: note.name,
    });

    const textarea = screen.getByLabelText("便签内容");
    expect(textarea).toHaveValue("稍后回来");
    fireEvent.change(textarea, { target: { value: "修订后的内容" } });
    fireEvent.click(screen.getByRole("button", { name: "保存便签" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("update_note_text", {
        documentId: "doc-1",
        name: "note-1",
        text: "修订后的内容",
      });
    });
    expect(onChanged).toHaveBeenCalled();
  });

  it("sends the ink line width with a style change", async () => {
    const { onChanged } = renderPanel({
      annotations: [ink],
      selectedName: ink.name,
    });

    const width = screen.getByLabelText("手绘线宽");
    expect(width).toHaveValue("6");
    fireEvent.change(width, { target: { value: "10" } });
    fireEvent.click(screen.getByRole("button", { name: "应用样式" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("update_annotation_style", {
        documentId: "doc-1",
        name: "ink-1",
        color: "#c62828",
        opacity: 80,
        lineWidth: 10,
      });
    });
    expect(onChanged).toHaveBeenCalled();
  });

  it("labels rectangles and ellipses in the annotation list", () => {
    const ellipse: AnnotationView = {
      ...rectangle,
      name: "s2",
      kind: "circle",
      pageIndex: 1,
      shape: { ...rectangle.shape!, kind: "circle" },
    };
    renderPanel({ annotations: [rectangle, ellipse] });

    expect(
      screen.getByRole("button", { name: "矩形，第 1 页，形状作者" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "椭圆，第 2 页，形状作者" }),
    ).toBeInTheDocument();
  });

  it("edits shape fill, stroke, opacity and line width with selection", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "update_shape_style") {
        return {
          annotation: rectangle,
          pageIndex: 0,
          history: idleHistory,
        };
      }
      throw new Error(`unexpected command: ${command}`);
    });
    const { onChanged } = renderPanel({
      annotations: [rectangle],
      selectedName: "s1",
    });

    expect(
      screen.getByText("矩形 · 尺寸 200 × 100 pt · 旋转 30°"),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText("注释颜色")).not.toBeInTheDocument();
    expect(screen.getByLabelText("形状填充颜色")).toHaveValue("#3366cc");
    expect(screen.getByLabelText("形状描边颜色")).toHaveValue("#000000");
    expect(screen.getByLabelText("形状线宽")).toHaveValue(3);
    expect(screen.getByLabelText("形状透明度")).toHaveValue("80");

    fireEvent.change(screen.getByLabelText("形状填充颜色"), {
      target: { value: "#ff0000" },
    });
    fireEvent.change(screen.getByLabelText("形状线宽"), {
      target: { value: "5" },
    });
    fireEvent.change(screen.getByLabelText("形状透明度"), {
      target: { value: "60" },
    });
    fireEvent.click(screen.getByLabelText("形状描边"));
    fireEvent.click(screen.getByRole("button", { name: "应用样式" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("update_shape_style", {
        documentId: "doc-1",
        name: "s1",
        style: {
          fillColor: "#ff0000",
          strokeColor: null,
          opacity: 60,
          lineWidth: 5,
        },
      });
    });
    expect(onChanged).toHaveBeenCalled();
  });

  it("refuses a shape style with neither fill nor stroke", () => {
    renderPanel({ annotations: [rectangle], selectedName: "s1" });
    fireEvent.click(screen.getByLabelText("形状填充"));
    fireEvent.click(screen.getByLabelText("形状描边"));
    expect(screen.getByRole("button", { name: "应用样式" })).toBeDisabled();
    expect(
      screen.getByText("至少需要填充或描边中的一项"),
    ).toBeInTheDocument();
  });
});
