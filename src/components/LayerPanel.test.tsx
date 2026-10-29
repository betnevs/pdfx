import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { EditHistoryView, LayerView } from "../lib/backend";
import { notifyDocumentChanged } from "../lib/documentEvents";
import { LayerPanel } from "./LayerPanel";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

const invokeMock = vi.mocked(invoke);

const red: LayerView = { index: 0, name: "红线", visible: true };
const blue: LayerView = { index: 1, name: "蓝线", visible: false };

const dirtyHistory: EditHistoryView = {
  canUndo: true,
  canRedo: false,
  revision: 1,
  savedRevision: 0,
  dirty: true,
};

type Handler = (args: unknown) => unknown;

function mockBackend(overrides: Record<string, Handler> = {}) {
  invokeMock.mockImplementation(async (command: string, args?: unknown) => {
    if (command === "list_layers") {
      return overrides.list_layers
        ? overrides.list_layers(args)
        : { documentId: "doc-1", layers: [red, blue] };
    }
    const handler = overrides[command];
    if (handler) {
      return handler(args);
    }
    throw new Error(`unexpected command: ${command}`);
  });
}

function commandCalls(command: string) {
  return invokeMock.mock.calls.filter(([name]) => name === command);
}

function renderPanel(
  overrides: Partial<{
    documentId: string;
    canModifyContent: boolean;
    readOnly: boolean;
    onHistory: (history: EditHistoryView) => void;
    onChanged: (repaint: boolean) => void;
  }> = {},
) {
  const onHistory = overrides.onHistory ?? vi.fn();
  const onChanged = overrides.onChanged ?? vi.fn();
  render(
    <LayerPanel
      documentId={overrides.documentId ?? "doc-1"}
      canModifyContent={overrides.canModifyContent ?? true}
      readOnly={overrides.readOnly ?? false}
      onHistory={onHistory}
      onChanged={onChanged}
    />,
  );
  return { onHistory, onChanged };
}

describe("LayerPanel", () => {
  beforeEach(() => {
    mockBackend();
  });

  afterEach(() => {
    invokeMock.mockReset();
  });

  it("lists the document's layers with their visibility", async () => {
    renderPanel();

    expect(await screen.findByText("红线")).toBeInTheDocument();
    expect(screen.getByText("蓝线")).toBeInTheDocument();
    expect(screen.getByText("共 2 个")).toBeInTheDocument();
    expect(
      screen.getByRole("checkbox", { name: "显示 红线" }),
    ).toBeChecked();
    expect(
      screen.getByRole("checkbox", { name: "显示 蓝线" }),
    ).not.toBeChecked();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows the empty state without an error when the document has no layers", async () => {
    mockBackend({
      list_layers: () => ({ documentId: "doc-1", layers: [] }),
    });
    renderPanel();

    expect(
      await screen.findByText("此文档没有可编辑图层"),
    ).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("toggles visibility, adopts the new list and publishes the history", async () => {
    mockBackend({
      set_layer_visible: (args) => {
        expect(args).toEqual({
          documentId: "doc-1",
          layerIndex: 0,
          visible: false,
        });
        return {
          layers: [{ ...red, visible: false }, blue],
          history: dirtyHistory,
        };
      },
    });
    const { onHistory, onChanged } = renderPanel();

    const checkbox = await screen.findByRole("checkbox", { name: "显示 红线" });
    fireEvent.click(checkbox);

    await waitFor(() =>
      expect(commandCalls("set_layer_visible")).toEqual([
        [
          "set_layer_visible",
          { documentId: "doc-1", layerIndex: 0, visible: false },
        ],
      ]),
    );
    await waitFor(() => expect(checkbox).not.toBeChecked());
    expect(onHistory).toHaveBeenCalledWith(dirtyHistory);
    expect(onChanged).toHaveBeenCalledWith(true);
  });

  it("shows a Chinese message when the layer is gone", async () => {
    mockBackend({
      set_layer_visible: () => {
        throw { code: "layer_not_found", message: "missing layer" };
      },
    });
    renderPanel();

    fireEvent.click(
      await screen.findByRole("checkbox", { name: "显示 红线" }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "该图层已不在文档中。",
    );
  });

  it("renames inline with trimming and one undoable step", async () => {
    mockBackend({
      rename_layer: (args) => {
        expect(args).toEqual({
          documentId: "doc-1",
          layerIndex: 0,
          name: "新红线",
        });
        return {
          layers: [{ ...red, name: "新红线" }, blue],
          history: dirtyHistory,
        };
      },
    });
    const { onHistory, onChanged } = renderPanel();

    await screen.findByText("红线");
    fireEvent.click(screen.getByRole("button", { name: "重命名 红线" }));
    const input = screen.getByLabelText("图层名称 红线");
    expect(input).toHaveValue("红线");
    fireEvent.change(input, { target: { value: "  新红线  " } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() =>
      expect(commandCalls("rename_layer")).toEqual([
        [
          "rename_layer",
          { documentId: "doc-1", layerIndex: 0, name: "新红线" },
        ],
      ]),
    );
    expect(await screen.findByText("新红线")).toBeInTheDocument();
    expect(onHistory).toHaveBeenCalledWith(dirtyHistory);
    expect(onChanged).toHaveBeenCalledWith(false);
    expect(
      screen.queryByLabelText("图层名称 红线"),
    ).not.toBeInTheDocument();
  });

  it("rejects blank and over-long names before calling the backend", async () => {
    renderPanel();

    await screen.findByText("红线");
    fireEvent.click(screen.getByRole("button", { name: "重命名 红线" }));
    const input = screen.getByLabelText("图层名称 红线");

    fireEvent.change(input, { target: { value: "   " } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "图层名称不能为空。",
    );
    expect(commandCalls("rename_layer")).toHaveLength(0);

    fireEvent.change(input, { target: { value: "名".repeat(121) } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "图层名称不能超过 120 个字符。",
    );
    expect(commandCalls("rename_layer")).toHaveLength(0);
  });

  it("cancels a rename with Escape", async () => {
    renderPanel();

    await screen.findByText("红线");
    fireEvent.click(screen.getByRole("button", { name: "重命名 红线" }));
    const input = screen.getByLabelText("图层名称 红线");
    fireEvent.change(input, { target: { value: "不要保存" } });
    fireEvent.keyDown(input, { key: "Escape" });

    expect(screen.queryByLabelText("图层名称 红线")).not.toBeInTheDocument();
    expect(screen.getByText("红线")).toBeInTheDocument();
    expect(commandCalls("rename_layer")).toHaveLength(0);
  });

  it("deletes only after confirmation and explains the content impact", async () => {
    mockBackend({
      delete_layer: (args) => {
        expect(args).toEqual({ documentId: "doc-1", layerIndex: 0 });
        return { layers: [blue], history: dirtyHistory };
      },
    });
    const { onHistory, onChanged } = renderPanel();

    await screen.findByText("红线");
    fireEvent.click(screen.getByRole("button", { name: "删除 红线" }));
    let dialog = screen.getByRole("dialog", { name: "删除图层？" });
    expect(dialog).toHaveTextContent("文字、图片等内容");
    expect(dialog).toHaveTextContent("撤销");
    expect(commandCalls("delete_layer")).toHaveLength(0);

    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(commandCalls("delete_layer")).toHaveLength(0);

    fireEvent.click(screen.getByRole("button", { name: "删除 红线" }));
    dialog = screen.getByRole("dialog", { name: "删除图层？" });
    fireEvent.click(within(dialog).getByRole("button", { name: "删除" }));

    await waitFor(() =>
      expect(commandCalls("delete_layer")).toEqual([
        ["delete_layer", { documentId: "doc-1", layerIndex: 0 }],
      ]),
    );
    await waitFor(() =>
      expect(screen.queryByText("红线")).not.toBeInTheDocument(),
    );
    expect(onHistory).toHaveBeenCalledWith(dirtyHistory);
    expect(onChanged).toHaveBeenCalledWith(true);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByText("蓝线")).toBeInTheDocument();
  });

  it("explains when deletion is refused as unsafe", async () => {
    mockBackend({
      delete_layer: () => {
        throw {
          code: "layer_delete_unsupported",
          message: "OCMD policy",
        };
      },
    });
    renderPanel();

    await screen.findByText("红线");
    fireEvent.click(screen.getByRole("button", { name: "删除 红线" }));
    fireEvent.click(
      within(screen.getByRole("dialog", { name: "删除图层？" })).getByRole(
        "button",
        { name: "删除" },
      ),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "该图层包含暂不支持安全删除的内容，未做任何更改。",
    );
  });

  it("reloads the list after an undo-style document change", async () => {
    let currentLayers = [red, blue];
    mockBackend({
      list_layers: () => ({ documentId: "doc-1", layers: currentLayers }),
    });
    renderPanel();

    const checkbox = await screen.findByRole("checkbox", { name: "显示 红线" });
    expect(checkbox).toBeChecked();
    expect(commandCalls("list_layers")).toHaveLength(1);

    currentLayers = [{ ...red, visible: false }, blue];
    act(() => {
      notifyDocumentChanged({
        documentId: "doc-1",
        pageIndex: 0,
        allPagesChanged: true,
      });
    });

    await waitFor(() => expect(checkbox).not.toBeChecked());
    expect(commandCalls("list_layers").length).toBeGreaterThan(1);
  });

  it("ignores document changes belonging to another document", async () => {
    renderPanel();
    await screen.findByText("红线");
    expect(commandCalls("list_layers")).toHaveLength(1);

    act(() => {
      notifyDocumentChanged({
        documentId: "doc-2",
        pageIndex: 0,
        allPagesChanged: true,
        source: "layer",
      });
    });

    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(commandCalls("list_layers")).toHaveLength(1);
  });

  it("disables editing with a Chinese reason for a read-only document", async () => {
    renderPanel({ canModifyContent: false, readOnly: true });

    expect(await screen.findByText("红线")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "显示 红线" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "重命名 红线" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "删除 红线" })).toBeDisabled();
    expect(
      screen.getByText("文档为只读，无法修改图层。"),
    ).toBeInTheDocument();
  });

  it("explains a permission restriction that is not a read-only document", async () => {
    renderPanel({ canModifyContent: false });

    expect(await screen.findByText("红线")).toBeInTheDocument();
    expect(
      screen.getByText("当前文档的权限不允许修改图层。"),
    ).toBeInTheDocument();
  });

  it("shows a Chinese message when the list cannot be loaded", async () => {
    mockBackend({
      list_layers: () => {
        throw { code: "document_not_found", message: "document closed" };
      },
    });
    renderPanel();

    expect(await screen.findByRole("alert")).toHaveTextContent("文档已关闭。");
    expect(
      screen.queryByText("此文档没有可编辑图层"),
    ).not.toBeInTheDocument();
  });
});
