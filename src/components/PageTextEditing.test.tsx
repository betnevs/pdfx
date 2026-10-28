import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  EditHistoryView,
  TextBoxTargetView,
  TextCharView,
  TextLayerView,
} from "../lib/backend";
import { DOCUMENT_CHANGED_EVENT } from "../lib/documentEvents";
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

const chars: TextCharView[] = [
  makeChar("H", 100, 100, 12),
  makeChar("i", 112, 100, 6),
  makeChar(" ", 118, 100, 6),
  makeChar("t", 124, 100, 6),
  makeChar("h", 130, 100, 8),
  makeChar("e", 138, 100, 8),
];

const textLayer: TextLayerView = {
  pageIndex: 2,
  pageWidth: 612,
  pageHeight: 792,
  rotation: 0,
  cropX: 0,
  cropY: 0,
  hasText: true,
  text: "Hi the",
  chars,
};

const scannedLayer: TextLayerView = { ...textLayer, hasText: false, text: "", chars: [] };

const emptyHistory: EditHistoryView = {
  canUndo: false,
  canRedo: false,
  revision: 0,
  savedRevision: 0,
  dirty: false,
};

const editedHistory: EditHistoryView = {
  canUndo: true,
  canRedo: false,
  revision: 1,
  savedRevision: 0,
  dirty: true,
};

const target: TextBoxTargetView = {
  boxId: 7,
  pageIndex: 2,
  text: "Hi the",
  fontFamily: "Helvetica",
  fontSize: 18,
  color: "#112233",
  x: 100,
  y: 100,
  width: 200,
  height: 20,
  bold: false,
  italic: false,
  underline: false,
  alignment: "left",
  rotation: 0,
  autoHeight: false,
  created: false,
};

type CommandHandler = (args: Record<string, unknown>) => unknown;

function mockCommands(overrides: Record<string, CommandHandler> = {}) {
  const handlers: Record<string, CommandHandler> = {
    extract_text_layer: () => textLayer,
    edit_history: () => emptyHistory,
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

async function renderLayer(readOnly = false) {
  const view = render(
    <PageTextLayer documentId="doc-1" pageIndex={2} readOnly={readOnly} scale={2} />,
  );
  await screen.findByRole("region", { name: "文本层" });
  return view;
}

async function enterEditMode() {
  fireEvent.click(screen.getByRole("button", { name: "编辑文字" }));
}

function clickCharacter() {
  const region = screen.getByRole("region", { name: "文本层" });
  fireEvent.pointerDown(region, {
    clientX: 102,
    clientY: 105,
    button: 0,
    pointerId: 1,
  });
  fireEvent.pointerUp(region, { clientX: 102, clientY: 105, pointerId: 1 });
}

describe("PageTextLayer text editing", () => {
  it("clicking text in edit mode prefills the Text Box from the original object", async () => {
    const targetCalls: Record<string, unknown>[] = [];
    mockCommands({
      text_edit_target: (args) => {
        targetCalls.push(args);
        return target;
      },
    });
    await renderLayer();
    await enterEditMode();
    clickCharacter();

    const textarea = await screen.findByRole("textbox", { name: "文字内容" });
    expect(targetCalls).toEqual([
      { documentId: "doc-1", pageIndex: 2, charIndex: 0 },
    ]);
    expect(textarea).toHaveValue("Hi the");
    expect(textarea).toHaveStyle({
      fontFamily: "Helvetica",
      color: "#112233",
      fontSize: "36px",
    });

    const editor = screen.getByTestId("text-box-editor");
    expect(parseFloat(editor.style.left)).toBeCloseTo((100 / 612) * 100, 5);
    expect(parseFloat(editor.style.top)).toBeCloseTo((100 / 792) * 100, 5);
    expect(parseFloat(editor.style.width)).toBeCloseTo((200 / 612) * 100, 5);
    expect(parseFloat(editor.style.minHeight)).toBeCloseTo(43.2, 5);

    // The editor is a live preview of what will be written.
    fireEvent.change(textarea, { target: { value: "Edited text" } });
    expect(textarea).toHaveValue("Edited text");
  });

  it("commits the edit, refreshes the page and updates undo availability", async () => {
    const applyCalls: Record<string, unknown>[] = [];
    const changed = vi.fn();
    const listener = (event: Event) =>
      changed((event as CustomEvent<{ documentId: string; pageIndex: number }>).detail);
    window.addEventListener(DOCUMENT_CHANGED_EVENT, listener);

    mockCommands({
      text_edit_target: () => target,
      apply_text_edit: (args) => {
        applyCalls.push(args);
        return {
          pageIndex: 2,
          boxId: 7,
          text: "Edited text",
          font: null,
          history: editedHistory,
        };
      },
    });
    await renderLayer();
    await enterEditMode();
    clickCharacter();

    const textarea = await screen.findByRole("textbox", { name: "文字内容" });
    fireEvent.change(textarea, { target: { value: "Edited text" } });
    fireEvent.click(screen.getByRole("button", { name: "应用" }));

    await waitFor(() =>
      expect(screen.queryByTestId("text-box-editor")).not.toBeInTheDocument(),
    );
    expect(applyCalls).toEqual([
      { documentId: "doc-1", pageIndex: 2, boxId: 7, text: "Edited text" },
    ]);
    expect(changed).toHaveBeenCalledWith({ documentId: "doc-1", pageIndex: 2 });
    await waitFor(() =>
      expect(
        invokeMock.mock.calls.filter(([command]) => command === "extract_text_layer"),
      ).toHaveLength(2),
    );
    expect(screen.getByRole("button", { name: "撤销" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "重做" })).toBeDisabled();

    window.removeEventListener(DOCUMENT_CHANGED_EVENT, listener);
  });

  it("cancels without writing anything", async () => {
    mockCommands({ text_edit_target: () => target });
    await renderLayer();
    await enterEditMode();
    clickCharacter();

    const textarea = await screen.findByRole("textbox", { name: "文字内容" });
    fireEvent.change(textarea, { target: { value: "Never committed" } });
    fireEvent.click(screen.getByRole("button", { name: "取消" }));

    expect(screen.queryByTestId("text-box-editor")).not.toBeInTheDocument();
    expect(
      invokeMock.mock.calls.some(([command]) => command === "apply_text_edit"),
    ).toBe(false);
  });

  it("deletes the Text Box through the backend", async () => {
    const deleteCalls: Record<string, unknown>[] = [];
    mockCommands({
      text_edit_target: () => target,
      delete_text_box: (args) => {
        deleteCalls.push(args);
        return {
          pageIndex: 2,
          boxId: 7,
          text: "",
          font: null,
          history: editedHistory,
        };
      },
    });
    await renderLayer();
    await enterEditMode();
    clickCharacter();
    await screen.findByRole("textbox", { name: "文字内容" });

    fireEvent.click(screen.getByRole("button", { name: "删除" }));

    await waitFor(() =>
      expect(screen.queryByTestId("text-box-editor")).not.toBeInTheDocument(),
    );
    expect(deleteCalls).toEqual([
      { documentId: "doc-1", pageIndex: 2, boxId: 7 },
    ]);
    expect(screen.getByRole("button", { name: "撤销" })).toBeEnabled();
  });

  it("reports a font fallback after committing", async () => {
    mockCommands({
      text_edit_target: () => target,
      apply_text_edit: () => ({
        pageIndex: 2,
        boxId: 7,
        text: "Edited text",
        font: {
          originalFamily: "PDFXMissingFont",
          family: "Helvetica",
          fallback: true,
          reason: "the original font PDFXMissingFont is not embedded",
        },
        history: editedHistory,
      }),
    });
    await renderLayer();
    await enterEditMode();
    clickCharacter();
    await screen.findByRole("textbox", { name: "文字内容" });
    fireEvent.click(screen.getByRole("button", { name: "应用" }));

    expect(
      await screen.findByText(/已改用 Helvetica/),
    ).toBeInTheDocument();
  });

  it("undoes and redoes from the toolbar", async () => {
    mockCommands({
      edit_history: () => ({ ...editedHistory, canRedo: true }),
      undo_text_edit: () => ({
        pageIndex: 2,
        history: { ...editedHistory, canUndo: false, canRedo: true },
      }),
      redo_text_edit: () => ({
        pageIndex: 2,
        history: { ...editedHistory, canUndo: true, canRedo: false },
      }),
    });
    await renderLayer();
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "撤销" })).toBeEnabled(),
    );

    fireEvent.click(screen.getByRole("button", { name: "撤销" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "重做" })).toBeEnabled(),
    );
    expect(screen.getByRole("button", { name: "撤销" })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "重做" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "撤销" })).toBeEnabled(),
    );

    expect(invokeMock).toHaveBeenCalledWith("undo_text_edit", {
      documentId: "doc-1",
    });
    expect(invokeMock).toHaveBeenCalledWith("redo_text_edit", {
      documentId: "doc-1",
    });
  });

  it("supports undo and redo keyboard shortcuts", async () => {
    mockCommands({
      edit_history: () => ({ ...editedHistory, canRedo: true }),
      undo_text_edit: () => ({
        pageIndex: 2,
        history: { ...editedHistory, canRedo: true },
      }),
      redo_text_edit: () => ({
        pageIndex: 2,
        history: { ...editedHistory, canRedo: true },
      }),
    });
    await renderLayer();
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "撤销" })).toBeEnabled(),
    );
    const region = screen.getByRole("region", { name: "文本层" });

    fireEvent.keyDown(region, { key: "z", ctrlKey: true });
    fireEvent.keyDown(region, { key: "z", ctrlKey: true, shiftKey: true });
    fireEvent.keyDown(region, { key: "y", metaKey: true });

    await waitFor(() => {
      const undo = invokeMock.mock.calls.filter(
        ([command]) => command === "undo_text_edit",
      );
      const redo = invokeMock.mock.calls.filter(
        ([command]) => command === "redo_text_edit",
      );
      expect(undo).toHaveLength(1);
      expect(redo).toHaveLength(2);
    });
  });

  it("disables text editing on scanned pages and explains why", async () => {
    mockCommands({ extract_text_layer: () => scannedLayer });
    render(<PageTextLayer documentId="doc-1" pageIndex={2} />);

    expect(await screen.findByText("此页没有可编辑文字")).toBeInTheDocument();
    const editButton = screen.getByRole("button", { name: "编辑文字" });
    expect(editButton).toBeDisabled();
    expect(editButton).toHaveAttribute("title", "此页没有可编辑文字");
  });

  it("disables editing with a visible reason on read-only documents", async () => {
    mockCommands();
    await renderLayer(true);

    const editButton = screen.getByRole("button", { name: "编辑文字" });
    expect(editButton).toBeDisabled();
    expect(editButton).toHaveAttribute("title", "此文档不允许编辑内容");
    expect(screen.getByText("此文档不允许编辑内容")).toBeInTheDocument();
    expect(
      invokeMock.mock.calls.some(([command]) => command === "text_edit_target"),
    ).toBe(false);
  });
});
