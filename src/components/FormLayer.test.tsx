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
});

afterEach(() => {
  invokeMock.mockReset();
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
      origin: "form",
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
});
