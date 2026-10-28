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
  rects: [{ x: 100, y: 100, width: 120, height: 20 }],
  stamp: null,
};

const underline: AnnotationView = {
  name: "a2",
  pageIndex: 1,
  kind: "underline",
  author: null,
  createdAt: null,
  color: "#00aa00",
  opacity: 100,
  rects: [{ x: 80, y: 200, width: 90, height: 18 }],
  stamp: null,
};

const stamp: AnnotationView = {
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

function renderPanel(overrides: Partial<Parameters<typeof AnnotationPanel>[0]> = {}) {
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
      });
    });
    expect(onChanged).toHaveBeenCalled();
  });

  it("shows stamp properties without a colour picker and keeps the stamp ink", async () => {
    const { onChanged } = renderPanel({
      annotations: [stamp],
      selectedName: "s1",
    });

    expect(
      screen.getByRole("button", {
        name: "图章 · 已批准，第 1 页，测试作者",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("图章：已批准")).toBeInTheDocument();
    expect(screen.queryByLabelText("注释颜色")).not.toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("注释透明度"), {
      target: { value: "40" },
    });
    fireEvent.click(screen.getByRole("button", { name: "应用样式" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("update_annotation_style", {
        documentId: "doc-1",
        name: "s1",
        color: "#15803d",
        opacity: 40,
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
      screen.getByText("选择页面文字添加标记，或从上方图章面板放置图章。"),
    ).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "第 1 页" })).not.toBeInTheDocument();
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
});
