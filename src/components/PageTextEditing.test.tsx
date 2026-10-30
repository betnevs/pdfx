import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  EditHistoryView,
  TextBoxTargetView,
  TextCharView,
  TextLayerView,
} from "../lib/backend";
import { flushPendingEdits } from "../lib/pendingEdits";
import { DOCUMENT_CHANGED_EVENT } from "../lib/documentEvents";
import { PageTextLayer } from "./PageTextLayer";
import { EditorToolsProvider, ToolChips, ToolPanelHost } from "./ToolShelf";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const invokeMock = vi.mocked(invoke);

function makeChar(
  text: string,
  x: number,
  y: number,
  width: number,
): TextCharView {
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

const scannedLayer: TextLayerView = {
  ...textLayer,
  hasText: false,
  text: "",
  chars: [],
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
  keepOriginalFont: false,
  runs: [],
};

type CommandHandler = (args: Record<string, unknown>) => unknown;

function mockCommands(overrides: Record<string, CommandHandler> = {}) {
  const handlers: Record<string, CommandHandler> = {
    extract_text_layer: () => textLayer,
    edit_history: () => emptyHistory,
    text_box_fonts: () => [],
    text_boxes: () => [],
    ...overrides,
  };
  invokeMock.mockImplementation((command, args) => {
    const handler = handlers[command as string];
    if (!handler) {
      return Promise.reject(
        new Error(`unexpected command ${command as string}`),
      );
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

async function renderLayer(readOnly = false, paintVersion?: number) {
  const view = render(
    <PageTextLayer
      documentId="doc-1"
      pageIndex={2}
      readOnly={readOnly}
      scale={2}
      paintVersion={paintVersion}
    />,
  );
  await screen.findByRole("region", { name: "文本层" });
  return view;
}

async function enterEditMode() {
  fireEvent.click(screen.getByRole("button", { name: "编辑文字" }));
}

function clickCharacter(edit = true) {
  const region = screen.getByRole("region", { name: "文本层" });
  fireEvent.pointerDown(region, {
    clientX: 102,
    clientY: 105,
    button: 0,
    pointerId: 1,
  });
  fireEvent.pointerUp(region, { clientX: 102, clientY: 105, pointerId: 1 });
  if (edit) fireEvent.doubleClick(region, { clientX: 102, clientY: 105 });
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
      color: "#112233",
      fontSize: "36px",
    });
    // The editor previews the PDF font through the webview's installed faces
    // instead of setting the unresolvable PDF family name.
    expect(textarea.style.fontFamily).toContain("Helvetica");
    expect(textarea.style.fontWeight).toBe("400");

    const editor = screen.getByTestId("text-box-editor");
    expect(parseFloat(editor.style.left)).toBeCloseTo((100 / 612) * 100, 5);
    expect(parseFloat(editor.style.top)).toBeCloseTo((100 / 792) * 100, 5);
    expect(parseFloat(editor.style.width)).toBeCloseTo((200 / 612) * 100, 4);
    expect(parseFloat(editor.style.minHeight)).toBeCloseTo(43.2, 5);
    // Actions float above the text instead of pushing it down, like the
    // reference editor's selection bar.
    expect(
      screen.getByRole("toolbar", { name: "文字编辑" }),
    ).toBeInTheDocument();
    for (const name of ["应用", "取消", "删除"]) {
      expect(screen.getByRole("button", { name })).toBeInTheDocument();
    }

    // The editor is a live preview of what will be written.
    fireEvent.change(textarea, { target: { value: "Edited text" } });
    expect(textarea).toHaveValue("Edited text");
  });

  it("offers the text style controls while editing original text", async () => {
    mockCommands({ text_edit_target: () => target });
    await renderLayer();
    await enterEditMode();
    clickCharacter();

    await screen.findByRole("textbox", { name: "文字内容" });
    expect(
      await screen.findByRole("combobox", { name: "字体" }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("字号")).toBeInTheDocument();
    expect(screen.getByLabelText("颜色")).toBeInTheDocument();
  });

  it("outlines editable text blocks in edit mode and marks the open one", async () => {
    mockCommands({ text_edit_target: () => target });
    await renderLayer();

    expect(screen.queryAllByTestId("text-edit-block")).toHaveLength(0);

    await enterEditMode();
    const blocks = screen.getAllByTestId("text-edit-block");
    expect(blocks).toHaveLength(1);
    expect(blocks[0]).not.toHaveClass("text-edit-block--active");
    expect(parseFloat(blocks[0].style.left)).toBeCloseTo((100 / 612) * 100, 5);
    expect(parseFloat(blocks[0].style.width)).toBeCloseTo((46 / 612) * 100, 5);

    clickCharacter();
    await screen.findByTestId("text-box-editor");
    expect(screen.getByTestId("text-box-editor")).toBeInTheDocument();
    expect(
      screen.queryByRole("group", { name: "选中文本框" }),
    ).not.toBeInTheDocument();
  });

  it("commits the edit, refreshes the page and updates undo availability", async () => {
    const applyCalls: Record<string, unknown>[] = [];
    const changed = vi.fn();
    const listener = (event: Event) =>
      changed(
        (event as CustomEvent<{ documentId: string; pageIndex: number }>)
          .detail,
      );
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
    expect(changed).toHaveBeenCalledWith({
      documentId: "doc-1",
      pageIndex: 2,
      source: "text",
    });
    await waitFor(() =>
      expect(
        invokeMock.mock.calls.filter(
          ([command]) => command === "extract_text_layer",
        ),
      ).toHaveLength(2),
    );
    expect(screen.getByRole("button", { name: "撤销" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "重做" })).toBeDisabled();

    window.removeEventListener(DOCUMENT_CHANGED_EVENT, listener);
  });

  it("shows form fills in the shared undo history", async () => {
    let history = emptyHistory;
    mockCommands({ edit_history: () => history });
    await renderLayer();

    expect(screen.getByRole("button", { name: "撤销" })).toBeDisabled();

    history = editedHistory;
    act(() => {
      window.dispatchEvent(
        new CustomEvent(DOCUMENT_CHANGED_EVENT, {
          detail: { documentId: "doc-1", pageIndex: 0, source: "form" },
        }),
      );
    });

    await waitFor(() =>
      expect(screen.getByRole("button", { name: "撤销" })).toBeEnabled(),
    );
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
    const input = await screen.findByRole("textbox", { name: "文字内容" });
    fireEvent.change(input, { target: { value: "Edited text" } });
    fireEvent.click(screen.getByRole("button", { name: "应用" }));

    expect(await screen.findByText(/已改用 Helvetica/)).toBeInTheDocument();
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

  it("leaves undo and redo shortcuts to the native menu", async () => {
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

    // The native menu owns Cmd/Ctrl+Z and Cmd/Ctrl+Shift+Z so one key press
    // produces exactly one history step. The layer must not run them a second
    // time, and must not hijack the textarea's native undo while editing.
    fireEvent.keyDown(region, { key: "z", ctrlKey: true });
    fireEvent.keyDown(region, { key: "z", ctrlKey: true, shiftKey: true });
    fireEvent.keyDown(region, { key: "y", metaKey: true });
    await new Promise((resolve) => setTimeout(resolve, 20));

    const undo = invokeMock.mock.calls.filter(
      ([command]) => command === "undo_text_edit",
    );
    const redo = invokeMock.mock.calls.filter(
      ([command]) => command === "redo_text_edit",
    );
    expect(undo).toHaveLength(0);
    expect(redo).toHaveLength(0);
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

it("commits the active textarea before document saving and refuses an invalid draft", async () => {
  const apply = vi.fn(() => ({
    pageIndex: 2,
    boxId: 7,
    text: "Saved directly",
    font: null,
    history: editedHistory,
  }));
  mockCommands({ text_edit_target: () => target, apply_text_edit: apply });
  await renderLayer();
  await enterEditMode();
  clickCharacter();
  const input = await screen.findByRole("textbox", { name: "文字内容" });
  fireEvent.change(input, { target: { value: "Saved directly" } });
  let success = false;
  await act(async () => {
    success = await flushPendingEdits("doc-1");
  });
  expect(success).toBe(true);
  expect(apply).toHaveBeenCalledWith(
    expect.objectContaining({ text: "Saved directly" }),
  );
  expect(
    screen.queryByRole("textbox", { name: "文字内容" }),
  ).not.toBeInTheDocument();
  clickCharacter();
  const blank = await screen.findByRole("textbox", { name: "文字内容" });
  fireEvent.change(blank, { target: { value: "" } });
  await act(async () => {
    success = await flushPendingEdits("doc-1");
  });
  expect(success).toBe(false);
  expect(apply).toHaveBeenCalledTimes(1);
});

describe("reference content selection", () => {
  it("selects original content on a single click, with eight resize handles and no textarea", async () => {
    mockCommands({ text_edit_target: () => target, text_boxes: () => [] });
    await renderLayer();
    await enterEditMode();
    clickCharacter(false);
    expect(
      await screen.findByRole("group", { name: "选中文本框" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("textbox", { name: "文字内容" }),
    ).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /调整文本框/ })).toHaveLength(
      8,
    );
    expect(screen.getByRole("button", { name: "编辑选中文字" })).toBeEnabled();
  });
});

it("moves selected original text with one transform and keeps the input closed", async () => {
  let box = { ...target };
  const transform = vi.fn((args: Record<string, unknown>) => {
    box = { ...box, ...(args.transform as object) };
    return { pageIndex: 2, target: box, font: null, history: editedHistory };
  });
  mockCommands({
    text_edit_target: () => box,
    text_boxes: () => [box],
    transform_text_box: transform,
  });
  await renderLayer();
  await enterEditMode();
  clickCharacter(false);
  const outline = await screen.findByRole("group", { name: "选中文本框" });
  fireEvent.pointerDown(outline, {
    button: 0,
    pointerId: 2,
    clientX: 120,
    clientY: 110,
  });
  fireEvent.pointerMove(outline, { pointerId: 2, clientX: 155, clientY: 150 });
  fireEvent.pointerUp(outline, { pointerId: 2, clientX: 155, clientY: 150 });
  await waitFor(() => expect(transform).toHaveBeenCalledTimes(1));
  expect(transform).toHaveBeenCalledWith(
    expect.objectContaining({
      boxId: 7,
      transform: expect.objectContaining({
        x: 135,
        y: 140,
        width: 200,
        height: 20,
      }),
    }),
  );
  expect(
    screen.queryByRole("textbox", { name: "文字内容" }),
  ).not.toBeInTheDocument();
});

it("resizes from the side without changing font size, and lets the native layout grow the height", async () => {
  let box = { ...target };
  const transform = vi.fn((args: Record<string, unknown>) => {
    box = { ...box, ...(args.transform as object), height: 64 };
    return { pageIndex: 2, target: box, font: null, history: editedHistory };
  });
  mockCommands({
    text_edit_target: () => box,
    text_boxes: () => [box],
    transform_text_box: transform,
  });
  await renderLayer();
  await enterEditMode();
  clickCharacter(false);
  const handle = await screen.findByRole("button", { name: "调整文本框 e" });
  fireEvent.pointerDown(handle, {
    button: 0,
    pointerId: 3,
    clientX: 300,
    clientY: 110,
  });
  fireEvent.pointerMove(handle, { pointerId: 3, clientX: 190, clientY: 110 });
  fireEvent.pointerUp(handle, { pointerId: 3, clientX: 190, clientY: 110 });
  await waitFor(() => expect(transform).toHaveBeenCalledTimes(1));
  expect(transform).toHaveBeenCalledWith(
    expect.objectContaining({
      transform: {
        x: 100,
        y: 100,
        width: 90,
        height: 21.599999999999998,
        autoHeight: true,
        rotation: 0,
      },
    }),
  );
  expect(box.fontSize).toBe(18);
  await waitFor(() =>
    expect(screen.getByRole("group", { name: "选中文本框" })).toHaveStyle({
      height: `${(70 / 792) * 100}%`,
    }),
  );
});

it("edits through the selection toolbar and commits on clicking blank page space", async () => {
  let box = { ...target };
  const apply = vi.fn((args: Record<string, unknown>) => {
    box = { ...box, text: String(args.text) };
    return {
      pageIndex: 2,
      boxId: 7,
      text: box.text,
      font: null,
      history: editedHistory,
    };
  });
  mockCommands({
    text_edit_target: () => box,
    text_boxes: () => [box],
    apply_text_edit: apply,
  });
  await renderLayer();
  await enterEditMode();
  clickCharacter(false);
  fireEvent.click(await screen.findByRole("button", { name: "编辑选中文字" }));
  const input = await screen.findByRole("textbox", { name: "文字内容" });
  expect(
    screen.queryByRole("toolbar", { name: "所选文字操作" }),
  ).not.toBeInTheDocument();
  fireEvent.change(input, { target: { value: "Changed paragraph" } });
  fireEvent.pointerDown(screen.getByRole("region", { name: "文本层" }), {
    button: 0,
    clientX: 450,
    clientY: 450,
  });
  await waitFor(() =>
    expect(apply).toHaveBeenCalledWith(
      expect.objectContaining({ text: "Changed paragraph" }),
    ),
  );
  await waitFor(() =>
    expect(
      screen.queryByRole("textbox", { name: "文字内容" }),
    ).not.toBeInTheDocument(),
  );
  expect(
    screen.queryByRole("group", { name: "选中文本框" }),
  ).not.toBeInTheDocument();
});

it("creates a paragraph in the workspace with selected placeholder text and commits by clicking outside", async () => {
  let box = {
    ...target,
    boxId: 30,
    text: "",
    created: true,
    fontSize: 12,
    x: 400,
    y: 400,
    width: 120,
    height: 32,
  };
  const create = vi.fn(() => ({
    pageIndex: 2,
    target: box,
    font: null,
    history: editedHistory,
  }));
  const apply = vi.fn((args: Record<string, unknown>) => {
    box = { ...box, text: String(args.text) };
    return {
      pageIndex: 2,
      boxId: 30,
      text: box.text,
      font: null,
      history: editedHistory,
    };
  });
  mockCommands({
    create_text_box: create,
    apply_text_edit: apply,
    text_boxes: () => [box],
  });
  render(
    <EditorToolsProvider group="text">
      <ToolChips />
      <ToolPanelHost />
      <PageTextLayer documentId="doc-1" pageIndex={2} scale={1} />
    </EditorToolsProvider>,
  );
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "编辑内容" })).toHaveAttribute(
      "aria-pressed",
      "true",
    ),
  );
  fireEvent.click(screen.getByRole("button", { name: "添加段落" }));
  const region = screen.getByRole("region", { name: "文本层" });
  fireEvent.pointerDown(region, {
    button: 0,
    pointerId: 5,
    clientX: 400,
    clientY: 400,
  });
  fireEvent.pointerUp(region, { pointerId: 5, clientX: 400, clientY: 400 });
  const input = await screen.findByRole("textbox", { name: "文字内容" });
  expect(input).toHaveValue("在这里插入文字");
  expect(create).toHaveBeenCalledWith(
    expect.objectContaining({
      request: expect.objectContaining({
        width: 120,
        style: expect.objectContaining({ fontSize: 12 }),
      }),
    }),
  );
  expect(
    screen.queryByRole("button", { name: "应用" }),
  ).not.toBeInTheDocument();
  fireEvent.change(input, { target: { value: "New paragraph" } });
  fireEvent.pointerDown(region, { button: 0, clientX: 550, clientY: 600 });
  await waitFor(() =>
    expect(apply).toHaveBeenCalledWith(
      expect.objectContaining({ text: "New paragraph" }),
    ),
  );
  await waitFor(() =>
    expect(
      screen.queryByRole("textbox", { name: "文字内容" }),
    ).not.toBeInTheDocument(),
  );
});

it("does not create an edit when a side handle is clicked or dragged only vertically", async () => {
  const box = { ...target, height: 100 };
  const transform = vi.fn();
  mockCommands({
    text_edit_target: () => box,
    text_boxes: () => [box],
    transform_text_box: transform,
  });
  await renderLayer();
  await enterEditMode();
  clickCharacter(false);
  const handle = await screen.findByRole("button", { name: "调整文本框 e" });
  fireEvent.pointerDown(handle, {
    button: 0,
    pointerId: 8,
    clientX: 300,
    clientY: 150,
  });
  fireEvent.pointerUp(handle, { pointerId: 8, clientX: 300, clientY: 150 });
  expect(transform).not.toHaveBeenCalled();
  fireEvent.pointerDown(handle, {
    button: 0,
    pointerId: 9,
    clientX: 300,
    clientY: 150,
  });
  fireEvent.pointerMove(handle, { pointerId: 9, clientX: 300, clientY: 190 });
  fireEvent.pointerUp(handle, { pointerId: 9, clientX: 300, clientY: 190 });
  expect(transform).not.toHaveBeenCalled();
});

it("commits the visible IME text when pointer-down happens before the final change event", async () => {
  const apply=vi.fn(()=>({pageIndex:2,boxId:7,text:"Hi the哈哈",font:null,history:editedHistory}));
  mockCommands({text_edit_target:()=>target,text_boxes:()=>[target],apply_text_edit:apply});
  await renderLayer();await enterEditMode();clickCharacter();
  const input=await screen.findByRole("textbox",{name:"文字内容"});
  fireEvent.compositionStart(input);
  // WebKit may update the live composing value before React receives change.
  (input as HTMLTextAreaElement).value="Hi the哈哈";
  fireEvent.pointerDown(screen.getByRole("region",{name:"文本层"}),{button:0,clientX:450,clientY:450});
  fireEvent.compositionEnd(input,{data:"哈哈"});
  await waitFor(()=>expect(apply).toHaveBeenCalledWith(expect.objectContaining({text:"Hi the哈哈"})));
});

it("uses full-sized handles even for a very short text line", async()=>{
  const short={...target,height:4};mockCommands({text_edit_target:()=>short,text_boxes:()=>[short]});
  await renderLayer();await enterEditMode();clickCharacter(false);
  await screen.findByRole("group",{name:"选中文本框"});
  for(const handle of screen.getAllByRole("button",{name:/调整文本框/})) expect(handle).toHaveStyle({width:"12px",height:"12px"});
});

it("captures the whole composing suffix before moving focus on outside click", async () => {
  const apply = vi.fn(() => ({pageIndex: 2, boxId: 7, text: "Hi there不错啊", font: null, history: editedHistory}));
  mockCommands({text_edit_target: () => target, text_boxes: () => [target], apply_text_edit: apply});
  await renderLayer(); await enterEditMode(); clickCharacter();
  const input = await screen.findByRole("textbox", {name: "文字内容"}) as HTMLTextAreaElement;
  fireEvent.change(input, {target: {value: "Hi there不"}});
  fireEvent.compositionStart(input);
  input.value = "Hi there不错啊";
  // Focus transfer can cancel WebKit's uncommitted suffix before change/end.
  input.addEventListener("blur", () => { input.value = "Hi there不"; });
  fireEvent.pointerDown(screen.getByRole("region", {name: "文本层"}), {button: 0, clientX: 450, clientY: 450});
  await waitFor(() => expect(apply).toHaveBeenCalledWith(expect.objectContaining({text: "Hi there不错啊"})));
});

it("moves cached native pixels without per-move IPC and holds the final position until paint", async () => {
  let box = {...target};
  const data = {
    background: {documentId: "doc-1", pageIndex: 2, scale: 2, width: 1224, height: 1584, pngBase64: "bg"},
    foreground: {documentId: "doc-1", pageIndex: 2, scale: 2, x: 200, y: 200, width: 400, height: 40, pngBase64: "text"},
  };
  const prepare = vi.fn(() => data);
  const expensivePreview = vi.fn();
  const transform = vi.fn((args: Record<string, unknown>) => {
    box = {...box, ...(args.transform as object)};
    return {pageIndex: 2, target: box, font: null, history: editedHistory};
  });
  mockCommands({text_edit_target: () => box, text_boxes: () => [box],
    prepare_text_box_move_preview: prepare, preview_text_box_transform: expensivePreview,
    transform_text_box: transform});
  const view = await renderLayer(false, 0); await enterEditMode(); clickCharacter(false);
  const outline = await screen.findByRole("group", {name: "选中文本框"});
  await waitFor(() => expect(prepare).toHaveBeenCalledTimes(1));
  fireEvent.pointerDown(outline, {button: 0, pointerId: 2, clientX: 120, clientY: 110});
  for (let i=1; i<=100; i++) fireEvent.pointerMove(outline, {pointerId: 2, clientX: 120+i, clientY: 110+i});
  await waitFor(() => expect(screen.getByTestId("text-move-sprite")).toHaveStyle({transform: "translate3d(200px, 200px, 0)"}));
  expect(prepare).toHaveBeenCalledTimes(1);
  expect(expensivePreview).not.toHaveBeenCalled();
  expect(transform).not.toHaveBeenCalled();
  // Release before a further animation frame: the exact final pointer wins.
  fireEvent.pointerUp(outline, {pointerId: 2, clientX: 245, clientY: 235});
  await waitFor(() => expect(transform).toHaveBeenCalledTimes(1));
  expect(transform).toHaveBeenCalledWith(expect.objectContaining({transform: expect.objectContaining({x: 225, y: 225})}));
  expect(screen.getByTestId("text-move-sprite")).toHaveStyle({transform: "translate3d(250px, 250px, 0)"});
  view.rerender(<PageTextLayer documentId="doc-1" pageIndex={2} scale={2} paintVersion={1} />);
  await waitFor(() => expect(screen.queryByTestId("text-move-sprite")).not.toBeInTheDocument());
});

it("cancels queued movement without resurrecting a drag or committing a transform", async () => {
  const transform = vi.fn();
  mockCommands({text_edit_target: () => target, text_boxes: () => [target],
    prepare_text_box_move_preview: () => null, transform_text_box: transform});
  await renderLayer(); await enterEditMode(); clickCharacter(false);
  const outline = await screen.findByRole("group", {name: "选中文本框"});
  fireEvent.pointerDown(outline, {button: 0, pointerId: 2, clientX: 120, clientY: 110});
  fireEvent.pointerMove(outline, {pointerId: 2, clientX: 220, clientY: 210});
  fireEvent.pointerCancel(outline, {pointerId: 2});
  await act(async () => {await new Promise(resolve => requestAnimationFrame(resolve));});
  fireEvent.pointerUp(outline, {pointerId: 2, clientX: 220, clientY: 210});
  expect(transform).not.toHaveBeenCalled();
  expect(screen.queryByTestId("text-move-sprite")).not.toBeInTheDocument();
});
