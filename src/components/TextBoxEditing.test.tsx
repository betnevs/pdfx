import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  EditHistoryView,
  TextBoxFontView,
  TextBoxTargetView,
  TextCharView,
  TextLayerView,
} from "../lib/backend";
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

const textLayer: TextLayerView = {
  pageIndex: 2,
  pageWidth: 612,
  pageHeight: 792,
  rotation: 0,
  cropX: 0,
  cropY: 0,
  hasText: true,
  text: "Original",
  chars: [
    makeChar("O", 72, 72, 12),
    makeChar("r", 84, 72, 7),
    makeChar("i", 91, 72, 4),
    makeChar("g", 95, 72, 9),
    makeChar("i", 104, 72, 4),
    makeChar("n", 108, 72, 9),
    makeChar("a", 117, 72, 9),
    makeChar("l", 126, 72, 4),
  ],
};

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

const FONTS: TextBoxFontView[] = ["Helvetica", "Times", "Courier"].flatMap(
  (family, familyIndex) =>
    [
      { bold: false, italic: false, postscriptName: family },
      { bold: true, italic: false, postscriptName: `${family}-Bold` },
      { bold: false, italic: true, postscriptName: `${family}-Italic` },
      { bold: true, italic: true, postscriptName: `${family}-BoldItalic` },
    ].map((variant, variantIndex) => ({
      family,
      label: familyIndex === 0 && variantIndex === 0 ? family : `${family} ${variantIndex}`,
      ...variant,
      source: "base14",
      embedded: false,
      embeddable: true,
      coverage: "win_ansi",
    })),
);

function makeBox(overrides: Partial<TextBoxTargetView> = {}): TextBoxTargetView {
  return {
    boxId: 1,
    pageIndex: 2,
    text: "",
    fontFamily: "Helvetica",
    fontSize: 16,
    color: "#000000",
    x: 10,
    y: 20,
    width: 200,
    height: 60,
    bold: false,
    italic: false,
    underline: false,
    alignment: "left",
    rotation: 0,
    autoHeight: true,
    created: true,
    ...overrides,
  };
}

type CommandHandler = (args: Record<string, unknown>) => unknown;

function mockCommands(overrides: Record<string, CommandHandler> = {}) {
  const handlers: Record<string, CommandHandler> = {
    extract_text_layer: () => textLayer,
    edit_history: () => emptyHistory,
    text_boxes: () => [],
    text_box_fonts: () => FONTS,
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
  render(
    <PageTextLayer documentId="doc-1" pageIndex={2} readOnly={readOnly} scale={1} />,
  );
  await screen.findByRole("region", { name: "文本层" });
}

async function enterBoxMode() {
  fireEvent.click(screen.getByRole("button", { name: "添加文本框" }));
}

describe("PageTextLayer text boxes", () => {
  it("creates a Text Box by dragging and writes typed text", async () => {
    const createCalls: Record<string, unknown>[] = [];
    const applyCalls: Record<string, unknown>[] = [];
    const store: TextBoxTargetView[] = [];
    const created = makeBox();

    mockCommands({
      text_boxes: () => [...store],
      create_text_box: (args) => {
        createCalls.push(args);
        store.push(created);
        return { pageIndex: 2, target: created, font: null, history: editedHistory };
      },
      apply_text_edit: (args) => {
        applyCalls.push(args);
        store[0] = { ...created, text: "Hello" };
        return {
          pageIndex: 2,
          boxId: created.boxId,
          text: "Hello",
          font: null,
          history: editedHistory,
        };
      },
    });

    await renderLayer();
    await enterBoxMode();

    const region = screen.getByRole("region", { name: "文本层" });
    fireEvent.pointerDown(region, {
      clientX: 10,
      clientY: 20,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerMove(region, {
      clientX: 210,
      clientY: 80,
      pointerId: 1,
    });
    fireEvent.pointerUp(region, {
      clientX: 210,
      clientY: 80,
      pointerId: 1,
    });

    await waitFor(() => expect(createCalls).toHaveLength(1));
    expect(createCalls[0]).toEqual({
      documentId: "doc-1",
      pageIndex: 2,
      request: { x: 10, y: 20, width: 200, height: 60 },
    });

    const textarea = await screen.findByRole("textbox", { name: "文字内容" });
    expect(textarea).toHaveValue("");
    fireEvent.change(textarea, { target: { value: "Hello" } });
    fireEvent.click(screen.getByRole("button", { name: "应用" }));

    await waitFor(() =>
      expect(screen.queryByTestId("text-box-editor")).not.toBeInTheDocument(),
    );
    expect(applyCalls).toEqual([
      { documentId: "doc-1", pageIndex: 2, boxId: created.boxId, text: "Hello" },
    ]);
    expect(
      await screen.findByRole("group", { name: "文本框 1" }),
    ).toBeInTheDocument();
  });

  it("moves a selected Text Box and records one undoable transform", async () => {
    const store = [makeBox({ boxId: 5, text: "Move me" })];
    const transformCalls: Record<string, unknown>[] = [];

    mockCommands({
      text_boxes: () => [...store],
      transform_text_box: (args) => {
        transformCalls.push(args);
        const transform = args.transform as Record<string, unknown>;
        store[0] = { ...store[0], ...transform };
        return {
          pageIndex: 2,
          target: store[0],
          font: null,
          history: editedHistory,
        };
      },
    });

    await renderLayer();
    await enterBoxMode();
    const outline = await screen.findByRole("group", { name: "文本框 1" });
    const region = screen.getByRole("region", { name: "文本层" });

    fireEvent.pointerDown(outline, {
      clientX: 110,
      clientY: 140,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerMove(region, {
      clientX: 140,
      clientY: 170,
      pointerId: 1,
    });
    fireEvent.pointerUp(region, {
      clientX: 140,
      clientY: 170,
      pointerId: 1,
    });

    await waitFor(() => expect(transformCalls).toHaveLength(1));
    expect(transformCalls[0]).toEqual({
      documentId: "doc-1",
      pageIndex: 2,
      boxId: 5,
      transform: {
        x: 40,
        y: 50,
        width: 200,
        height: 60,
        autoHeight: true,
        rotation: 0,
      },
    });
    await waitFor(() =>
      expect(parseFloat(outline.style.left)).toBeCloseTo((40 / 612) * 100, 5),
    );
    expect(parseFloat(outline.style.top)).toBeCloseTo((50 / 792) * 100, 5);
  });

  it("resizes a Text Box by hand and turns off auto height", async () => {
    const store = [makeBox({ boxId: 5, text: "Resize me" })];
    const transformCalls: Record<string, unknown>[] = [];

    mockCommands({
      text_boxes: () => [...store],
      transform_text_box: (args) => {
        transformCalls.push(args);
        const transform = args.transform as Record<string, unknown>;
        store[0] = { ...store[0], ...transform };
        return {
          pageIndex: 2,
          target: store[0],
          font: null,
          history: editedHistory,
        };
      },
    });

    await renderLayer();
    await enterBoxMode();
    const outline = await screen.findByRole("group", { name: "文本框 1" });
    fireEvent.pointerDown(outline, {
      clientX: 110,
      clientY: 140,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerUp(outline, {
      clientX: 110,
      clientY: 140,
      pointerId: 1,
    });

    const handle = await screen.findByTestId("text-box-resize-handle");
    const region = screen.getByRole("region", { name: "文本层" });
    fireEvent.pointerDown(handle, {
      clientX: 210,
      clientY: 80,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerMove(region, {
      clientX: 260,
      clientY: 130,
      pointerId: 1,
    });
    fireEvent.pointerUp(region, {
      clientX: 260,
      clientY: 130,
      pointerId: 1,
    });

    await waitFor(() => expect(transformCalls).toHaveLength(1));
    expect(transformCalls[0]).toEqual({
      documentId: "doc-1",
      pageIndex: 2,
      boxId: 5,
      transform: {
        x: 10,
        y: 20,
        width: 250,
        height: 110,
        autoHeight: false,
        rotation: 0,
      },
    });
    await waitFor(() =>
      expect(parseFloat(outline.style.width)).toBeCloseTo((250 / 612) * 100, 5),
    );
  });

  it("rotates a Text Box and keeps its size", async () => {
    const store = [makeBox({ boxId: 5, text: "Rotate me" })];
    const transformCalls: Record<string, unknown>[] = [];

    mockCommands({
      text_boxes: () => [...store],
      transform_text_box: (args) => {
        transformCalls.push(args);
        const transform = args.transform as Record<string, unknown>;
        store[0] = { ...store[0], ...transform };
        return {
          pageIndex: 2,
          target: store[0],
          font: null,
          history: editedHistory,
        };
      },
    });

    await renderLayer();
    await enterBoxMode();
    const outline = await screen.findByRole("group", { name: "文本框 1" });
    fireEvent.pointerDown(outline, {
      clientX: 110,
      clientY: 140,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerUp(outline, {
      clientX: 110,
      clientY: 140,
      pointerId: 1,
    });

    const handle = await screen.findByTestId("text-box-rotate-handle");
    const region = screen.getByRole("region", { name: "文本层" });
    fireEvent.pointerDown(handle, {
      clientX: 210,
      clientY: 20,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerMove(region, {
      clientX: 10,
      clientY: 220,
      pointerId: 1,
    });
    fireEvent.pointerUp(region, {
      clientX: 10,
      clientY: 220,
      pointerId: 1,
    });

    await waitFor(() => expect(transformCalls).toHaveLength(1));
    expect(transformCalls[0]).toEqual({
      documentId: "doc-1",
      pageIndex: 2,
      boxId: 5,
      transform: {
        x: 10,
        y: 20,
        width: 200,
        height: 60,
        autoHeight: true,
        rotation: 90,
      },
    });
    await waitFor(() =>
      expect(outline.style.transform).toBe("rotate(90deg)"),
    );
  });

  it("grows an auto-height box for overflowing text without shrinking it", async () => {
    const longText =
      "The quick brown fox jumps over the lazy dog. ".repeat(6) +
      "\nExplicit newline stays.";
    const store = [
      makeBox({ boxId: 5, text: "Short", height: 60, autoHeight: true }),
    ];
    const applyCalls: Record<string, unknown>[] = [];

    mockCommands({
      text_boxes: () => [...store],
      apply_text_edit: (args) => {
        applyCalls.push(args);
        store[0] = { ...store[0], text: args.text as string, height: 240 };
        return {
          pageIndex: 2,
          boxId: 5,
          text: args.text as string,
          font: null,
          history: editedHistory,
        };
      },
    });

    await renderLayer();
    await enterBoxMode();
    const outline = await screen.findByRole("group", { name: "文本框 1" });
    fireEvent.doubleClick(outline);

    const editor = await screen.findByTestId("text-box-editor");
    const textarea = screen.getByRole("textbox", { name: "文字内容" });
    const before = parseFloat(editor.style.minHeight);
    fireEvent.change(textarea, { target: { value: longText } });
    const after = parseFloat(editor.style.minHeight);
    expect(after).toBeGreaterThan(before);

    fireEvent.click(screen.getByRole("button", { name: "应用" }));
    await waitFor(() => expect(applyCalls).toHaveLength(1));
    expect(applyCalls[0]).toEqual({
      documentId: "doc-1",
      pageIndex: 2,
      boxId: 5,
      text: longText,
    });
    await waitFor(() =>
      expect(parseFloat(outline.style.height)).toBeCloseTo((240 / 792) * 100, 5),
    );
  });

  it("creates an empty box with a click, deletes it, and undo restores it", async () => {
    const created = makeBox();
    const store: TextBoxTargetView[] = [];
    const createCalls: Record<string, unknown>[] = [];
    const deleteCalls: Record<string, unknown>[] = [];

    mockCommands({
      text_boxes: () => [...store],
      edit_history: () => editedHistory,
      create_text_box: (args) => {
        createCalls.push(args);
        store.push(created);
        return {
          pageIndex: 2,
          target: created,
          font: null,
          history: editedHistory,
        };
      },
      delete_text_box: (args) => {
        deleteCalls.push(args);
        store.length = 0;
        return {
          pageIndex: 2,
          boxId: created.boxId,
          text: "",
          font: null,
          history: editedHistory,
        };
      },
      undo_text_edit: () => {
        store.push(created);
        return {
          pageIndex: 2,
          history: { ...editedHistory, canUndo: false, canRedo: true },
        };
      },
      redo_text_edit: () => {
        store.length = 0;
        return {
          pageIndex: 2,
          history: { ...editedHistory, canUndo: true, canRedo: false },
        };
      },
    });

    await renderLayer();
    await enterBoxMode();
    const region = screen.getByRole("region", { name: "文本层" });
    fireEvent.pointerDown(region, {
      clientX: 100,
      clientY: 100,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerUp(region, {
      clientX: 100,
      clientY: 100,
      pointerId: 1,
    });

    await waitFor(() => expect(createCalls).toHaveLength(1));
    expect(createCalls[0]).toEqual({
      documentId: "doc-1",
      pageIndex: 2,
      request: { x: 100, y: 100, width: 200, height: 32 },
    });
    expect(await screen.findByRole("textbox", { name: "文字内容" })).toHaveValue(
      "",
    );

    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(screen.getByRole("group", { name: "文本框 1" })).toBeInTheDocument();
    expect(screen.getByText("空文本框")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "删除文本框" }));
    await waitFor(() => expect(deleteCalls).toHaveLength(1));
    expect(deleteCalls[0]).toEqual({
      documentId: "doc-1",
      pageIndex: 2,
      boxId: created.boxId,
    });
    await waitFor(() =>
      expect(
        screen.queryByRole("group", { name: "文本框 1" }),
      ).not.toBeInTheDocument(),
    );

    fireEvent.click(screen.getByRole("button", { name: "撤销" }));
    expect(
      await screen.findByRole("group", { name: "文本框 1" }),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "重做" }));
    await waitFor(() =>
      expect(
        screen.queryByRole("group", { name: "文本框 1" }),
      ).not.toBeInTheDocument(),
    );
    expect(invokeMock).toHaveBeenCalledWith("undo_text_edit", {
      documentId: "doc-1",
    });
    expect(invokeMock).toHaveBeenCalledWith("redo_text_edit", {
      documentId: "doc-1",
    });
  });

  it("disables the Text Box tool on read-only documents", async () => {
    mockCommands();
    await renderLayer(true);
    const button = screen.getByRole("button", { name: "添加文本框" });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute("title", "此文档不允许编辑内容");
    expect(
      invokeMock.mock.calls.some(([command]) => command === "text_boxes"),
    ).toBe(false);
  });

  it("edits the whole-box style from the base-14 font list", async () => {
    const store = [makeBox({ boxId: 5, text: "Styled" })];
    const styleCalls: Record<string, unknown>[] = [];

    mockCommands({
      text_boxes: () => [...store],
      text_box_fonts: () => FONTS,
      apply_text_box_style: (args) => {
        styleCalls.push(args);
        const style = args.style as Record<string, unknown>;
        store[0] = {
          ...store[0],
          fontFamily: style.fontFamily as string,
          fontSize: style.fontSize as number,
          bold: style.bold as boolean,
          italic: style.italic as boolean,
          underline: style.underline as boolean,
          color: style.color as string,
          alignment: style.alignment as TextBoxTargetView["alignment"],
        };
        return {
          pageIndex: 2,
          target: store[0],
          font: null,
          history: editedHistory,
        };
      },
    });

    await renderLayer();
    await enterBoxMode();
    const outline = await screen.findByRole("group", { name: "文本框 1" });
    fireEvent.pointerDown(outline, {
      clientX: 110,
      clientY: 140,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerUp(outline, {
      clientX: 110,
      clientY: 140,
      pointerId: 1,
    });

    const family = await screen.findByRole("combobox", { name: "字体" });
    expect(
      [...(family as HTMLSelectElement).options].map((option) => option.value),
    ).toEqual(["Helvetica", "Times", "Courier"]);
    expect(screen.getByText(/仅支持 PDF 基础字体/)).toBeInTheDocument();
    expect(screen.queryByText(/TrueType/)).not.toBeInTheDocument();

    fireEvent.change(family, { target: { value: "Times" } });
    await waitFor(() => expect(styleCalls).toHaveLength(1));
    expect(styleCalls[0]).toEqual({
      documentId: "doc-1",
      pageIndex: 2,
      boxId: 5,
      style: {
        fontFamily: "Times",
        fontSize: 16,
        bold: false,
        italic: false,
        underline: false,
        color: "#000000",
        alignment: "left",
      },
    });

    fireEvent.change(screen.getByRole("spinbutton", { name: "字号" }), {
      target: { value: "24" },
    });
    await waitFor(() => expect(styleCalls).toHaveLength(2));
    expect(styleCalls[1]).toMatchObject({
      boxId: 5,
      style: { fontFamily: "Times", fontSize: 24 },
    });

    fireEvent.click(screen.getByRole("button", { name: "粗体" }));
    await waitFor(() => expect(styleCalls).toHaveLength(3));
    expect(styleCalls[2]).toMatchObject({ style: { bold: true } });

    fireEvent.click(screen.getByRole("button", { name: "斜体" }));
    await waitFor(() => expect(styleCalls).toHaveLength(4));
    expect(styleCalls[3]).toMatchObject({ style: { italic: true } });

    fireEvent.click(screen.getByRole("button", { name: "下划线" }));
    await waitFor(() => expect(styleCalls).toHaveLength(5));
    expect(styleCalls[4]).toMatchObject({ style: { underline: true } });

    fireEvent.change(screen.getByLabelText("颜色"), {
      target: { value: "#c0392b" },
    });
    await waitFor(() => expect(styleCalls).toHaveLength(6));
    expect(styleCalls[5]).toMatchObject({ style: { color: "#c0392b" } });

    fireEvent.click(screen.getByRole("button", { name: "居中" }));
    await waitFor(() => expect(styleCalls).toHaveLength(7));
    expect(styleCalls[6]).toMatchObject({ style: { alignment: "center" } });
  });
});
