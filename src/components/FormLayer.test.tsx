import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { notifyDocumentChanged } from "../lib/documentEvents";
import type { FormFieldView } from "../lib/backend";
import { FormLayer } from "./FormLayer";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const invokeMock = vi.mocked(invoke);

function field(overrides: Partial<FormFieldView> = {}): FormFieldView {
  return {
    fieldId: "0:1",
    name: "full_name",
    kind: "text",
    pageIndex: 0,
    order: 0,
    rect: { x: 72, y: 72, width: 180, height: 18 },
    value: null,
    checked: false,
    options: [],
    selectedIndex: null,
    required: false,
    readOnly: false,
    multiline: false,
    radioGroup: null,
    exportValue: null,
    tooltip: null,
    fontSize: null,
    created: false,
    ...overrides,
  };
}

const fields: FormFieldView[] = [
  field({ fieldId: "0:1", name: "full_name", order: 0, required: true }),
  field({
    fieldId: "0:2",
    name: "subscribe",
    kind: "checkbox",
    order: 1,
    rect: { x: 72, y: 100, width: 12, height: 12 },
  }),
  field({
    fieldId: "0:3",
    name: "gender",
    kind: "radio",
    order: 2,
    rect: { x: 72, y: 120, width: 12, height: 12 },
    radioGroup: "gender",
    exportValue: "male",
  }),
  field({
    fieldId: "0:4",
    name: "gender",
    kind: "radio",
    order: 3,
    rect: { x: 92, y: 120, width: 12, height: 12 },
    radioGroup: "gender",
    exportValue: "female",
    checked: true,
    value: "female",
  }),
  field({
    fieldId: "0:5",
    name: "country",
    kind: "listBox",
    order: 4,
    rect: { x: 72, y: 140, width: 120, height: 18 },
    options: ["China", "Japan", "Germany"],
    value: "Japan",
    selectedIndex: 1,
  }),
  field({
    fieldId: "0:6",
    name: "department",
    kind: "comboBox",
    order: 5,
    rect: { x: 72, y: 160, width: 120, height: 18 },
    options: ["Engineering", "Design", "Sales"],
  }),
  field({
    fieldId: "0:7",
    name: "readonly_note",
    kind: "text",
    order: 6,
    rect: { x: 72, y: 180, width: 120, height: 18 },
    value: "Read only",
    readOnly: true,
  }),
  field({
    fieldId: "1:1",
    name: "notes",
    kind: "text",
    pageIndex: 1,
    order: 7,
    rect: { x: 72, y: 72, width: 300, height: 60 },
    multiline: true,
  }),
];

const emptyHistory = {
  canUndo: false,
  canRedo: false,
  revision: 0,
  savedRevision: 0,
  dirty: false,
};

const dirtyHistory = {
  canUndo: true,
  canRedo: false,
  revision: 1,
  savedRevision: 0,
  dirty: true,
};

const createdField = field({
  fieldId: "new:1",
  name: "text_field_1",
  order: 0,
  rect: { x: 72, y: 100, width: 180, height: 24 },
  tooltip: null,
  fontSize: 12,
  created: true,
});

const originalGetBoundingClientRect = Element.prototype.getBoundingClientRect;

function installFields(list: FormFieldView[] = fields) {
  invokeMock.mockImplementation(async (command: string) => {
    if (command === "form_fields") {
      return list;
    }
    if (command === "fill_form_field") {
      return { fieldId: "0:1", fields: list, history: emptyHistory };
    }
    throw new Error(`unexpected command: ${command}`);
  });
}

function renderLayer(overrides: Partial<Parameters<typeof FormLayer>[0]> = {}) {
  return render(
    <FormLayer
      documentId="doc-1"
      pageIndex={0}
      pageWidth={612}
      pageHeight={792}
      canFillForms
      {...overrides}
    />,
  );
}

beforeEach(() => {
  installFields();
  // The overlay maps client coordinates through its rendered page box. jsdom
  // has no layout, so every element reports the page-sized box.
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

describe("FormLayer", () => {
  it("renders each widget of the current page in document order", async () => {
    const { container } = renderLayer();

    const input = await screen.findByLabelText("full_name");
    expect(input).toHaveValue("");
    expect(input.closest(".form-field")).toHaveStyle({
      left: `${(72 / 612) * 100}%`,
      top: `${(72 / 792) * 100}%`,
    });
    expect(input.closest(".form-field")).toHaveClass("form-field--required-missing");

    expect(screen.getByLabelText("subscribe")).not.toBeChecked();
    expect(screen.getByLabelText("gender（male）")).not.toBeChecked();
    expect(screen.getByLabelText("gender（female）")).toBeChecked();
    expect(screen.getByLabelText("country")).toHaveValue("Japan");
    expect(screen.getByLabelText("department")).toHaveValue("");
    expect(screen.queryByLabelText("notes")).not.toBeInTheDocument();

    // Read-only widgets stay visible but cannot take input.
    expect(screen.getByLabelText("readonly_note")).toBeDisabled();

    expect(container.querySelectorAll(".form-field")).toHaveLength(7);
  });

  it("commits text on blur and keeps the value the backend returns", async () => {
    const changed = fields.map((item) =>
      item.fieldId === "0:1" ? { ...item, value: "Ada Lovelace" } : item,
    );
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === "form_fields") {
        return fields;
      }
      if (command === "fill_form_field") {
        expect(args).toEqual({
          documentId: "doc-1",
          fieldId: "0:1",
          value: { kind: "text", text: "Ada Lovelace" },
        });
        return { fieldId: "0:1", fields: changed, history: emptyHistory };
      }
      throw new Error(`unexpected command: ${command}`);
    });
    const onDocumentChanged = vi.fn();
    const listener = (event: Event) => {
      const detail = (event as CustomEvent).detail;
      if (detail?.documentId === "doc-1") {
        onDocumentChanged(detail);
      }
    };
    window.addEventListener("pdfx://document-changed", listener);

    renderLayer();
    const input = await screen.findByLabelText("full_name");
    fireEvent.change(input, { target: { value: "Ada Lovelace" } });
    fireEvent.blur(input);

    await waitFor(() => expect(input).toHaveValue("Ada Lovelace"));
    expect(onDocumentChanged).toHaveBeenCalledWith({
      documentId: "doc-1",
      pageIndex: 0,
      source: "form",
    });
    window.removeEventListener("pdfx://document-changed", listener);
  });

  it("toggles checkboxes, selects radio widgets and chooses list values", async () => {
    const calls: Array<{ fieldId: string; value: unknown }> = [];
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === "form_fields") {
        return fields;
      }
      if (command === "fill_form_field") {
        const payload = args as { fieldId: string; value: unknown };
        calls.push({ fieldId: payload.fieldId, value: payload.value });
        return { fieldId: payload.fieldId, fields, history: emptyHistory };
      }
      throw new Error(`unexpected command: ${command}`);
    });

    renderLayer();
    fireEvent.click(await screen.findByLabelText("subscribe"));
    fireEvent.click(screen.getByLabelText("gender（male）"));
    fireEvent.change(screen.getByLabelText("country"), {
      target: { value: "Germany" },
    });
    fireEvent.change(screen.getByLabelText("department"), {
      target: { value: "Design" },
    });

    await waitFor(() => expect(calls).toHaveLength(4));
    expect(calls[0]).toEqual({
      fieldId: "0:2",
      value: { kind: "checked", checked: true },
    });
    expect(calls[1]).toEqual({
      fieldId: "0:3",
      value: { kind: "checked", checked: true },
    });
    expect(calls[2]).toEqual({
      fieldId: "0:5",
      value: { kind: "choice", value: "Germany" },
    });
    expect(calls[3]).toEqual({
      fieldId: "0:6",
      value: { kind: "choice", value: "Design" },
    });
  });

  it("commits the draft and moves focus to the next field when Tab is pressed", async () => {
    const calls: Array<{ fieldId: string; value: unknown }> = [];
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === "form_fields") {
        return fields;
      }
      if (command === "fill_form_field") {
        const payload = args as { fieldId: string; value: unknown };
        calls.push({ fieldId: payload.fieldId, value: payload.value });
        return { fieldId: payload.fieldId, fields, history: emptyHistory };
      }
      throw new Error(`unexpected command: ${command}`);
    });

    renderLayer();
    const input = await screen.findByLabelText("full_name");
    input.focus();
    fireEvent.change(input, { target: { value: "Ada" } });
    fireEvent.keyDown(input, { key: "Tab" });

    expect(screen.getByLabelText("subscribe")).toHaveFocus();
    await waitFor(() =>
      expect(calls).toEqual([
        { fieldId: "0:1", value: { kind: "text", text: "Ada" } },
      ]),
    );
  });

  it("moves to the first field of the next page when Tab runs past this page", async () => {
    const onSelectPage = vi.fn();
    const onFocusField = vi.fn();
    renderLayer({ onSelectPage, onFocusField });

    // `department` is the last editable widget of page 0; the read-only note
    // is skipped and `notes` on page 1 is next.
    const select = await screen.findByLabelText("department");
    select.focus();
    fireEvent.keyDown(select, { key: "Tab" });

    expect(onSelectPage).toHaveBeenCalledWith(1);
    expect(onFocusField).toHaveBeenCalledWith("1:1");
  });

  it("disables every widget when the document forbids filling forms", async () => {
    renderLayer({ canFillForms: false });

    expect(await screen.findByLabelText("full_name")).toBeDisabled();
    expect(screen.getByLabelText("subscribe")).toBeDisabled();
    expect(screen.getByLabelText("country")).toBeDisabled();
    expect(screen.getByText("此文档不允许填写表单")).toBeInTheDocument();
  });

  it("focuses the requested field after a required-field check navigates", async () => {
    const { rerender } = renderLayer();
    await screen.findByLabelText("full_name");

    const handled = vi.fn();
    rerender(
      <FormLayer
        documentId="doc-1"
        pageIndex={0}
        pageWidth={612}
        pageHeight={792}
        canFillForms
        focusFieldId="0:2"
        onFocusHandled={handled}
      />,
    );

    await waitFor(() =>
      expect(screen.getByLabelText("subscribe")).toHaveFocus(),
    );
    expect(handled).toHaveBeenCalledTimes(1);
  });

  it("reloads field values after another edit tool announces a change", async () => {
    renderLayer();
    expect(await screen.findByLabelText("full_name")).toHaveValue("");

    const updated = fields.map((item) =>
      item.fieldId === "0:1" ? { ...item, value: "Restored" } : item,
    );
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "form_fields") {
        return updated;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    notifyDocumentChanged({ documentId: "doc-1", pageIndex: 0 });

    await waitFor(() =>
      expect(screen.getByLabelText("full_name")).toHaveValue("Restored"),
    );
  });

  it("places a text field by dragging and opens its property panel", async () => {
    let stored: FormFieldView[] = [];
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === "form_fields") {
        return stored;
      }
      if (command === "edit_history") {
        return emptyHistory;
      }
      if (command === "create_form_text_field") {
        expect(args).toEqual({
          documentId: "doc-1",
          pageIndex: 0,
          request: {
            rect: { x: 72, y: 100, width: 180, height: 24 },
            name: null,
            tooltip: null,
            required: false,
            multiline: false,
            fontSize: 12,
          },
        });
        stored = [createdField];
        return { fieldId: "new:1", fields: stored, history: dirtyHistory };
      }
      throw new Error(`unexpected command: ${command}`);
    });
    const { container } = renderLayer({ canModifyContent: true });

    const toggle = await screen.findByRole("button", {
      name: "添加文本字段",
    });
    expect(toggle).toBeEnabled();
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-pressed", "true");

    const layer = container.querySelector(".form-layer");
    expect(layer).not.toBeNull();
    fireEvent.pointerDown(layer!, {
      clientX: 72,
      clientY: 100,
      button: 0,
      pointerId: 1,
    });
    fireEvent.pointerMove(layer!, {
      clientX: 252,
      clientY: 124,
      pointerId: 1,
    });
    fireEvent.pointerUp(layer!, {
      clientX: 252,
      clientY: 124,
      pointerId: 1,
    });

    expect(await screen.findByLabelText("字段名称")).toHaveValue(
      "text_field_1",
    );
    expect(screen.getByLabelText("字段提示")).toHaveValue("");
    expect(screen.getByLabelText("必填字段")).not.toBeChecked();
    expect(screen.getByLabelText("多行文本")).not.toBeChecked();
    expect(screen.getByLabelText("字号")).toHaveValue(12);
    // The widget is fillable through the existing fill path right away.
    expect(await screen.findByLabelText("text_field_1")).toBeEnabled();
    expect(toggle).toHaveAttribute("aria-pressed", "false");
  });

  it("updates created field properties from the panel", async () => {
    const updates: unknown[] = [];
    let stored: FormFieldView[] = [createdField];
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === "form_fields") {
        return stored;
      }
      if (command === "edit_history") {
        return emptyHistory;
      }
      if (command === "update_form_text_field") {
        updates.push(args);
        const payload = args as {
          fieldId: string;
          request: {
            name: string;
            tooltip: string | null;
            required: boolean;
            multiline: boolean;
            fontSize: number;
          };
        };
        const current = stored.find(
          (field) => field.fieldId === payload.fieldId,
        );
        stored = [
          {
            ...(current ?? createdField),
            ...payload.request,
            tooltip: payload.request.tooltip,
          },
        ];
        return { fields: stored, history: dirtyHistory };
      }
      throw new Error(`unexpected command: ${command}`);
    });
    renderLayer({ canModifyContent: true });

    fireEvent.click(await screen.findByRole("button", { name: "字段属性" }));
    const name = await screen.findByLabelText("字段名称");
    fireEvent.change(name, { target: { value: "customer_name" } });
    fireEvent.blur(name);
    await waitFor(() => expect(updates).toHaveLength(1));
    expect(updates[0]).toEqual({
      documentId: "doc-1",
      fieldId: "new:1",
      request: {
        name: "customer_name",
        tooltip: null,
        required: false,
        multiline: false,
        fontSize: 12,
      },
    });

    const tooltip = screen.getByLabelText("字段提示");
    fireEvent.change(tooltip, { target: { value: "请输入客户姓名" } });
    fireEvent.blur(tooltip);
    await waitFor(() => expect(updates).toHaveLength(2));

    fireEvent.click(screen.getByLabelText("必填字段"));
    await waitFor(() => expect(updates).toHaveLength(3));
    fireEvent.click(screen.getByLabelText("多行文本"));
    await waitFor(() => expect(updates).toHaveLength(4));

    const fontSize = screen.getByLabelText("字号");
    fireEvent.change(fontSize, { target: { value: "16" } });
    fireEvent.blur(fontSize);
    await waitFor(() => expect(updates).toHaveLength(5));
    expect(updates[4]).toEqual({
      documentId: "doc-1",
      fieldId: "new:1",
      request: {
        name: "customer_name",
        tooltip: "请输入客户姓名",
        required: true,
        multiline: true,
        fontSize: 16,
      },
    });

    // The refreshed widget carries the new name and kind without a reload.
    const widget = await screen.findByLabelText("customer_name");
    expect(widget.tagName).toBe("TEXTAREA");
    expect(widget).toBeEnabled();
  });

  it("keeps the field unchanged when the backend rejects a duplicate name", async () => {
    let updateCalls = 0;
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "form_fields") {
        return [createdField];
      }
      if (command === "edit_history") {
        return emptyHistory;
      }
      if (command === "update_form_text_field") {
        updateCalls += 1;
        throw {
          code: "form_field_name_duplicate",
          message: "表单字段名称“full_name”已存在，请换一个名称。",
        };
      }
      throw new Error(`unexpected command: ${command}`);
    });
    renderLayer({ canModifyContent: true });

    fireEvent.click(await screen.findByRole("button", { name: "字段属性" }));
    const name = await screen.findByLabelText("字段名称");
    fireEvent.change(name, { target: { value: "full_name" } });
    fireEvent.blur(name);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "表单字段名称“full_name”已存在，请换一个名称。",
    );
    expect(updateCalls).toBe(1);
    // The draft is kept so the user can correct it, and no second field was
    // created under the duplicate name.
    expect(name).toHaveValue("full_name");
    expect(screen.getByLabelText("text_field_1")).toBeInTheDocument();
  });

  it("deletes a created field and restores it with undo", async () => {
    let stored: FormFieldView[] = [createdField];
    let deleted = false;
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === "form_fields") {
        return stored;
      }
      if (command === "edit_history") {
        return deleted ? dirtyHistory : emptyHistory;
      }
      if (command === "delete_form_text_field") {
        expect(args).toEqual({ documentId: "doc-1", fieldId: "new:1" });
        deleted = true;
        stored = [];
        return { fields: [], history: dirtyHistory };
      }
      if (command === "undo_text_edit") {
        expect(args).toEqual({ documentId: "doc-1" });
        deleted = false;
        stored = [createdField];
        return {
          pageIndex: 0,
          history: {
            ...dirtyHistory,
            canUndo: false,
            canRedo: true,
            revision: 2,
          },
        };
      }
      throw new Error(`unexpected command: ${command}`);
    });
    renderLayer({ canModifyContent: true });

    fireEvent.click(await screen.findByRole("button", { name: "字段属性" }));
    fireEvent.click(await screen.findByRole("button", { name: "删除字段" }));

    await waitFor(() =>
      expect(screen.queryByLabelText("text_field_1")).not.toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "撤销" }));

    expect(await screen.findByLabelText("text_field_1")).toBeInTheDocument();
  });

  it("disables creation with a Chinese reason when content changes are forbidden", async () => {
    installFields([]);
    renderLayer({ canModifyContent: false });

    const button = await screen.findByRole("button", {
      name: "添加文本字段",
    });
    expect(button).toBeDisabled();
    expect(
      screen.getByText("此文档的权限限制不允许创建表单字段"),
    ).toBeInTheDocument();
  });
});
