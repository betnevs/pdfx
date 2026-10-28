import { useCallback, useEffect, useRef, useState } from "react";
import type {
  KeyboardEvent as ReactKeyboardEvent,
  PointerEvent as ReactPointerEvent,
} from "react";

import {
  createFormTextField,
  deleteFormTextField,
  editHistory,
  fillFormField,
  formFields,
  redoTextEdit,
  toCommandError,
  undoTextEdit,
  updateFormTextField,
  type CreateFormTextFieldRequest,
  type EditHistoryView,
  type FormFieldRect,
  type FormFieldView,
  type FormFieldValueInput,
  type UpdateFormTextFieldRequest,
} from "../lib/backend";
import { notifyDocumentChanged, onDocumentChanged } from "../lib/documentEvents";

/** Kinds the overlay can display and fill; buttons and signatures are inert. */
const FILLABLE_KINDS: ReadonlySet<FormFieldView["kind"]> = new Set([
  "text",
  "checkbox",
  "radio",
  "comboBox",
  "listBox",
]);

const FORMS_DISABLED_REASON = "此文档不允许填写表单";
const LOAD_FAILED_REASON = "表单字段无法载入";
const STRUCTURE_DISABLED_REASON = "此文档的权限限制不允许创建表单字段";
const REQUIRED_MARK = "必填";
const DEFAULT_FIELD_WIDTH = 180;
const DEFAULT_FIELD_HEIGHT = 24;
/** A drag shorter than this in either direction uses the default size. */
const MIN_DRAG_SIZE = 12;
const DEFAULT_FONT_SIZE = 12;

interface PagePoint {
  x: number;
  y: number;
}

interface FieldPanelDraft {
  fieldId: string;
  name: string;
  tooltip: string;
  required: boolean;
  multiline: boolean;
  /** Kept as text so an invalid entry can be reported instead of lost. */
  fontSize: string;
}

function draftFor(field: FormFieldView): FieldPanelDraft {
  return {
    fieldId: field.fieldId,
    name: field.name,
    tooltip: field.tooltip ?? "",
    required: field.required,
    multiline: field.multiline,
    fontSize: String(field.fontSize ?? DEFAULT_FONT_SIZE),
  };
}

function sameDraft(draft: FieldPanelDraft, field: FormFieldView): boolean {
  return (
    draft.name === field.name &&
    draft.tooltip.trim() === (field.tooltip ?? "") &&
    draft.required === field.required &&
    draft.multiline === field.multiline &&
    Number.parseFloat(draft.fontSize) ===
      (field.fontSize ?? DEFAULT_FONT_SIZE)
  );
}

/** Clamps a proposed placement rectangle to the page and enforces the minimum. */
function placementRect(
  anchor: PagePoint,
  current: PagePoint | null,
  pageWidth: number,
  pageHeight: number,
): FormFieldRect {
  const dragged =
    current !== null &&
    Math.abs(current.x - anchor.x) >= MIN_DRAG_SIZE &&
    Math.abs(current.y - anchor.y) >= MIN_DRAG_SIZE;
  let width = dragged
    ? Math.abs(current.x - anchor.x)
    : Math.min(DEFAULT_FIELD_WIDTH, pageWidth);
  let height = dragged
    ? Math.abs(current.y - anchor.y)
    : Math.min(DEFAULT_FIELD_HEIGHT, pageHeight);
  width = Math.min(width, pageWidth);
  height = Math.min(height, pageHeight);
  let x = dragged ? Math.min(anchor.x, current.x) : anchor.x;
  let y = dragged ? Math.min(anchor.y, current.y) : anchor.y;
  x = Math.min(Math.max(x, 0), Math.max(pageWidth - width, 0));
  y = Math.min(Math.max(y, 0), Math.max(pageHeight - height, 0));
  return { x, y, width, height };
}

export interface FormLayerProps {
  documentId: string;
  pageIndex: number;
  /** Display-space page size in points, matching the rendered page box. */
  pageWidth: number;
  pageHeight: number;
  /** The document's Permission Restrictions allow changing form values. */
  canFillForms: boolean;
  /** The document's Permission Restrictions allow changing content, which
   * includes creating, editing and deleting Form Fields. */
  canModifyContent?: boolean;
  /** Page render scale, used to size the overlaid text controls. */
  scale?: number;
  /** Widget to focus after the caller navigated to it. */
  focusFieldId?: string | null;
  /** Called once a requested widget received focus. */
  onFocusHandled?: () => void;
  /** Moves the viewer to another page (used by document-wide Tab order). */
  onSelectPage?: (pageIndex: number) => void;
  /** Requests focus for a widget on another page. */
  onFocusField?: (fieldId: string) => void;
}

function labelFor(field: FormFieldView): string {
  if (field.kind === "radio" && field.exportValue) {
    return `${field.name}（${field.exportValue}）`;
  }
  return field.name;
}

/**
 * Transparent, clickable overlay for the existing AcroForm widgets of one
 * page.
 *
 * Widget rectangles come from the backend in display-space points, so the
 * overlay positions them as percentages of the page surface and stays aligned
 * at every zoom level. Values commit through `fill_form_field`, which records
 * the change in the shared undo/redo history; the returned field list becomes
 * the new state. JavaScript and calculation actions are never executed.
 *
 * Read-only widgets stay visible but cannot be edited, and when the document
 * does not permit filling forms every control is disabled with a stated
 * reason.
 */
export function FormLayer({
  documentId,
  pageIndex,
  pageWidth,
  pageHeight,
  canFillForms,
  canModifyContent = true,
  scale = 1,
  focusFieldId,
  onFocusHandled,
  onSelectPage,
  onFocusField,
}: FormLayerProps) {
  const [fields, setFields] = useState<FormFieldView[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [busyFieldId, setBusyFieldId] = useState<string | null>(null);
  const [history, setHistory] = useState<EditHistoryView | null>(null);
  const [placing, setPlacing] = useState(false);
  const [placementStart, setPlacementStart] = useState<PagePoint | null>(null);
  const [placementCurrent, setPlacementCurrent] = useState<PagePoint | null>(null);
  const [structureBusy, setStructureBusy] = useState(false);
  const [selectedFieldId, setSelectedFieldId] = useState<string | null>(null);
  const [panelDraft, setPanelDraft] = useState<FieldPanelDraft | null>(null);
  const [panelError, setPanelError] = useState<string | null>(null);
  const layerRef = useRef<HTMLDivElement>(null);
  const controls = useRef(new Map<string, HTMLElement>());
  const inFlight = useRef(new Set<string>());
  const loadSequence = useRef(0);
  const lastFocusedRequest = useRef<string | null>(null);

  const reload = useCallback(() => {
    const sequence = ++loadSequence.current;
    formFields(documentId)
      .then((next) => {
        if (sequence === loadSequence.current) {
          setFields(next);
          setLoadError(null);
          setLoaded(true);
        }
      })
      .catch((error: unknown) => {
        if (sequence === loadSequence.current) {
          setLoadError(toCommandError(error).message);
          setLoaded(true);
        }
      });
  }, [documentId]);

  useEffect(() => {
    loadSequence.current += 1;
    setFields([]);
    setDrafts({});
    setNotice(null);
    setLoaded(false);
    setLoadError(null);
    setPlacing(false);
    setPlacementStart(null);
    setPlacementCurrent(null);
    setSelectedFieldId(null);
    setPanelDraft(null);
    setPanelError(null);
    reload();
    return () => {
      // Invalidate responses from the previous document.
      loadSequence.current += 1;
    };
  }, [documentId, pageIndex, reload]);

  // The shared history drives the form toolbar's undo/redo. It is refreshed
  // after this layer's own commands (which return it directly) and whenever a
  // different tool announces a change.
  useEffect(() => {
    let cancelled = false;
    editHistory(documentId)
      .then((next) => {
        if (!cancelled) {
          setHistory(next);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setHistory(null);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [documentId, pageIndex]);

  // Drop the property panel when undo, redo or a deletion removes its field.
  useEffect(() => {
    setSelectedFieldId((current) =>
      current !== null && fields.some((field) => field.fieldId === current)
        ? current
        : null,
    );
  }, [fields]);

  useEffect(() => {
    if (!selectedFieldId) {
      setPanelDraft(null);
      setPanelError(null);
    }
  }, [selectedFieldId]);

  // Undo, redo or another tool's edit can change field values; the tool that
  // made a form change already applied the command result, so its own event
  // only refreshes the other views.
  useEffect(
    () =>
      onDocumentChanged((detail) => {
        if (detail.documentId !== documentId) {
          return;
        }
        if (detail.source !== "form") {
          reload();
          editHistory(documentId)
            .then((next) => setHistory(next))
            .catch(() => {});
        }
      }),
    [documentId, reload],
  );

  const setControlRef = (fieldId: string) => (element: HTMLElement | null) => {
    if (element) {
      controls.current.set(fieldId, element);
    } else {
      controls.current.delete(fieldId);
    }
  };

  const clearDraft = (fieldId: string) => {
    setDrafts((current) => {
      if (!(fieldId in current)) {
        return current;
      }
      const next = { ...current };
      delete next[fieldId];
      return next;
    });
  };

  const commit = useCallback(
    async (field: FormFieldView, value: FormFieldValueInput) => {
      if (!canFillForms || field.readOnly || inFlight.current.has(field.fieldId)) {
        return;
      }
      inFlight.current.add(field.fieldId);
      setBusyFieldId(field.fieldId);
      setNotice(null);
      try {
        const result = await fillFormField(documentId, field.fieldId, value);
        setFields(result.fields);
        setLoaded(true);
        clearDraft(field.fieldId);
        notifyDocumentChanged({
          documentId,
          pageIndex: field.pageIndex,
          source: "form",
        });
      } catch (error) {
        clearDraft(field.fieldId);
        setNotice(toCommandError(error).message);
      } finally {
        inFlight.current.delete(field.fieldId);
        setBusyFieldId(null);
      }
    },
    [canFillForms, documentId],
  );

  const pageFields = fields.filter(
    (field) => field.pageIndex === pageIndex && FILLABLE_KINDS.has(field.kind),
  );
  const createdPageFields = fields.filter(
    (field) => field.pageIndex === pageIndex && field.created,
  );
  const selectedField =
    selectedFieldId === null
      ? null
      : (fields.find(
          (field) => field.fieldId === selectedFieldId && field.created,
        ) ?? null);

  const pointFromEvent = (event: {
    clientX: number;
    clientY: number;
  }): PagePoint | null => {
    const layer = layerRef.current;
    if (!layer || pageWidth <= 0 || pageHeight <= 0) {
      return null;
    }
    const box = layer.getBoundingClientRect();
    if (box.width <= 0 || box.height <= 0) {
      return null;
    }
    const x = ((event.clientX - box.left) / box.width) * pageWidth;
    const y = ((event.clientY - box.top) / box.height) * pageHeight;
    return {
      x: Math.min(Math.max(x, 0), pageWidth),
      y: Math.min(Math.max(y, 0), pageHeight),
    };
  };

  function togglePlacing() {
    if (!canModifyContent || structureBusy) {
      return;
    }
    setPlacing((current) => !current);
    setPlacementStart(null);
    setPlacementCurrent(null);
    setNotice(null);
    setPanelError(null);
  }

  function selectField(fieldId: string) {
    const field = fields.find((candidate) => candidate.fieldId === fieldId);
    if (!field) {
      return;
    }
    setSelectedFieldId(fieldId);
    setPanelDraft(draftFor(field));
    setPanelError(null);
  }

  function openPropertyPanel() {
    if (selectedField) {
      setSelectedFieldId(null);
      return;
    }
    const first = createdPageFields[0];
    if (first) {
      selectField(first.fieldId);
    }
  }

  async function placeField(rect: FormFieldRect) {
    setStructureBusy(true);
    setNotice(null);
    try {
      const request: CreateFormTextFieldRequest = {
        rect,
        name: null,
        tooltip: null,
        required: false,
        multiline: false,
        fontSize: DEFAULT_FONT_SIZE,
      };
      const result = await createFormTextField(documentId, pageIndex, request);
      setFields(result.fields);
      setHistory(result.history);
      setLoaded(true);
      setPlacing(false);
      setSelectedFieldId(result.fieldId);
      const created = result.fields.find(
        (field) => field.fieldId === result.fieldId,
      );
      setPanelDraft(created ? draftFor(created) : null);
      setPanelError(null);
      notifyDocumentChanged({ documentId, pageIndex, source: "form" });
    } catch (error) {
      setNotice(toCommandError(error).message);
    } finally {
      setStructureBusy(false);
    }
  }

  async function commitPanel(next: FieldPanelDraft | null = panelDraft) {
    const field = selectedField;
    if (!field || !next) {
      return;
    }
    if (structureBusy) {
      return;
    }
    if (sameDraft(next, field)) {
      setPanelDraft(draftFor(field));
      setPanelError(null);
      return;
    }
    const fontSize = Number.parseFloat(next.fontSize);
    if (!Number.isFinite(fontSize) || fontSize < 4 || fontSize > 200) {
      setPanelError("字号必须在 4 到 200 点之间。");
      return;
    }
    setStructureBusy(true);
    setPanelError(null);
    setNotice(null);
    try {
      const request: UpdateFormTextFieldRequest = {
        name: next.name,
        tooltip: next.tooltip.trim() === "" ? null : next.tooltip,
        required: next.required,
        multiline: next.multiline,
        fontSize,
      };
      const result = await updateFormTextField(
        documentId,
        field.fieldId,
        request,
      );
      setFields(result.fields);
      setHistory(result.history);
      const updated = result.fields.find(
        (candidate) => candidate.fieldId === field.fieldId,
      );
      if (updated) {
        setPanelDraft(draftFor(updated));
      }
      notifyDocumentChanged({ documentId, pageIndex, source: "form" });
    } catch (error) {
      setPanelError(toCommandError(error).message);
    } finally {
      setStructureBusy(false);
    }
  }

  async function deleteSelectedField() {
    const field = selectedField;
    if (!field || structureBusy) {
      return;
    }
    setStructureBusy(true);
    setNotice(null);
    try {
      const result = await deleteFormTextField(documentId, field.fieldId);
      setFields(result.fields);
      setHistory(result.history);
      setSelectedFieldId(null);
      setPanelDraft(null);
      setPanelError(null);
      notifyDocumentChanged({ documentId, pageIndex, source: "form" });
    } catch (error) {
      setNotice(toCommandError(error).message);
    } finally {
      setStructureBusy(false);
    }
  }

  async function runStructureHistory(kind: "undo" | "redo") {
    if (
      (kind === "undo" ? !history?.canUndo : !history?.canRedo) ||
      structureBusy
    ) {
      return;
    }
    setStructureBusy(true);
    setNotice(null);
    setPlacing(false);
    setSelectedFieldId(null);
    setPanelDraft(null);
    try {
      const step =
        kind === "undo"
          ? await undoTextEdit(documentId)
          : await redoTextEdit(documentId);
      setHistory(step.history);
      const next = await formFields(documentId);
      setFields(next);
      setLoaded(true);
      setLoadError(null);
      notifyDocumentChanged({
        documentId,
        pageIndex: step.pageIndex,
        source: "form",
      });
    } catch (error) {
      setNotice(toCommandError(error).message);
    } finally {
      setStructureBusy(false);
    }
  }

  const handleStructurePointerDown = (
    event: ReactPointerEvent<HTMLDivElement>,
  ) => {
    if (!placing || structureBusy || event.button !== 0) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    setPlacementStart(point);
    setPlacementCurrent(point);
    try {
      event.currentTarget.setPointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture is unavailable in some embedded webviews.
    }
  };

  const handleStructurePointerMove = (
    event: ReactPointerEvent<HTMLDivElement>,
  ) => {
    if (!placing || placementStart === null) {
      return;
    }
    const point = pointFromEvent(event);
    if (point) {
      setPlacementCurrent(point);
    }
  };

  const handleStructurePointerUp = (
    event: ReactPointerEvent<HTMLDivElement>,
  ) => {
    if (!placing || placementStart === null) {
      return;
    }
    const current = pointFromEvent(event) ?? placementCurrent;
    const start = placementStart;
    setPlacementStart(null);
    setPlacementCurrent(null);
    try {
      event.currentTarget.releasePointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture may never have been acquired.
    }
    void placeField(placementRect(start, current, pageWidth, pageHeight));
  };

  // Focus a widget the caller asked for (required-field validation, or the
  // next page during document-wide Tab order). The control must already be
  // mounted; otherwise this runs again after the page change.
  useEffect(() => {
    if (!focusFieldId) {
      lastFocusedRequest.current = null;
      return;
    }
    if (lastFocusedRequest.current === focusFieldId) {
      return;
    }
    const control = controls.current.get(focusFieldId);
    if (!control) {
      return;
    }
    lastFocusedRequest.current = focusFieldId;
    control.focus();
    onFocusHandled?.();
  }, [focusFieldId, pageIndex, fields, onFocusHandled]);

  const commitText = (field: FormFieldView, text: string) => {
    if (text === (field.value ?? "")) {
      clearDraft(field.fieldId);
      return;
    }
    void commit(field, { kind: "text", text });
  };

  const focusRelative = (field: FormFieldView, delta: number): boolean => {
    const orderable = fields.filter(
      (candidate) =>
        FILLABLE_KINDS.has(candidate.kind) && !candidate.readOnly,
    );
    const index = orderable.findIndex(
      (candidate) => candidate.fieldId === field.fieldId,
    );
    const target = index >= 0 ? orderable[index + delta] : undefined;
    if (!target) {
      return false;
    }
    if (target.pageIndex === pageIndex) {
      const control = controls.current.get(target.fieldId);
      if (!control) {
        return false;
      }
      control.focus();
      return true;
    }
    onFocusField?.(target.fieldId);
    onSelectPage?.(target.pageIndex);
    return Boolean(onSelectPage);
  };

  const handleKeyDown = (
    event: ReactKeyboardEvent<HTMLElement>,
    field: FormFieldView,
    text: string,
  ) => {
    if (event.key === "Escape") {
      event.preventDefault();
      clearDraft(field.fieldId);
      return;
    }
    if (field.kind === "text") {
      const multiline = field.multiline;
      if (event.key === "Enter") {
        if (multiline && !(event.metaKey || event.ctrlKey)) {
          return;
        }
        event.preventDefault();
        commitText(field, text);
        return;
      }
      if (event.key === "Tab") {
        if (focusRelative(field, event.shiftKey ? -1 : 1)) {
          event.preventDefault();
        }
        // The focus change blurs this control, which commits the draft; the
        // in-flight guard makes this an idempotent second attempt.
        commitText(field, text);
        return;
      }
    }
    if (event.key === "Tab") {
      if (focusRelative(field, event.shiftKey ? -1 : 1)) {
        event.preventDefault();
      }
    }
  };

  if (!loaded) {
    return null;
  }

  const interactive = (field: FormFieldView) =>
    canFillForms && !field.readOnly && busyFieldId !== field.fieldId;

  const rectStyle = (field: FormFieldView) => ({
    left: `${(field.rect.x / pageWidth) * 100}%`,
    top: `${(field.rect.y / pageHeight) * 100}%`,
    width: `${(field.rect.width / pageWidth) * 100}%`,
    height: `${(field.rect.height / pageHeight) * 100}%`,
  });

  const classNameFor = (field: FormFieldView, kind: string) => {
    const requiredMissing =
      field.required &&
      !field.readOnly &&
      ((field.kind === "text" && !(field.value ?? "").trim()) ||
        (field.kind === "checkbox" && !field.checked) ||
        (field.kind === "radio" && field.value === null) ||
        ((field.kind === "comboBox" || field.kind === "listBox") &&
          !(field.value ?? "").trim()));
    return [
      "form-field",
      `form-field--${kind}`,
      requiredMissing ? "form-field--required-missing" : "",
      field.readOnly ? "form-field--read-only" : "",
      !canFillForms ? "form-field--disabled" : "",
    ]
      .filter(Boolean)
      .join(" ");
  };

  const titleFor = (field: FormFieldView) =>
    [
      field.name,
      field.required ? REQUIRED_MARK : "",
      field.readOnly ? "只读" : "",
      !canFillForms ? FORMS_DISABLED_REASON : "",
    ]
      .filter(Boolean)
      .join(" · ");

  const renderControl = (field: FormFieldView) => {
    const common = {
      "aria-label": labelFor(field),
      title: titleFor(field),
      style: rectStyle(field),
    };
    switch (field.kind) {
      case "text": {
        const value = drafts[field.fieldId] ?? field.value ?? "";
        const style = {
          ...rectStyle(field),
          fontSize: `${Math.max(field.rect.height * scale * 0.62, 8)}px`,
        };
        if (field.multiline) {
          return (
            <textarea
              key={field.fieldId}
              ref={setControlRef(field.fieldId)}
              className={classNameFor(field, "multiline")}
              aria-label={common["aria-label"]}
              title={common.title}
              style={style}
              rows={1}
              value={value}
              disabled={!interactive(field)}
              spellCheck={false}
              onChange={(event) =>
                setDrafts((current) => ({
                  ...current,
                  [field.fieldId]: event.target.value,
                }))
              }
              onBlur={() => commitText(field, value)}
              onKeyDown={(event) => handleKeyDown(event, field, value)}
            />
          );
        }
        return (
          <input
            key={field.fieldId}
            ref={setControlRef(field.fieldId)}
            type="text"
            className={classNameFor(field, "text")}
            aria-label={common["aria-label"]}
            title={common.title}
            style={style}
            value={value}
            disabled={!interactive(field)}
            spellCheck={false}
            onChange={(event) =>
              setDrafts((current) => ({
                ...current,
                [field.fieldId]: event.target.value,
              }))
            }
            onBlur={() => commitText(field, value)}
            onKeyDown={(event) => handleKeyDown(event, field, value)}
          />
        );
      }
      case "checkbox":
        return (
          <input
            key={field.fieldId}
            ref={setControlRef(field.fieldId)}
            type="checkbox"
            className={classNameFor(field, "checkbox")}
            aria-label={common["aria-label"]}
            title={common.title}
            style={rectStyle(field)}
            checked={field.checked}
            disabled={!interactive(field)}
            onChange={() =>
              void commit(field, {
                kind: "checked",
                checked: !field.checked,
              })
            }
            onKeyDown={(event) => handleKeyDown(event, field, "")}
          />
        );
      case "radio":
        return (
          <input
            key={field.fieldId}
            ref={setControlRef(field.fieldId)}
            type="radio"
            className={classNameFor(field, "radio")}
            name={`pdfx-radio-${documentId}-${field.radioGroup ?? field.name}`}
            aria-label={common["aria-label"]}
            title={common.title}
            style={rectStyle(field)}
            checked={field.checked}
            disabled={!interactive(field)}
            onChange={() =>
              void commit(field, { kind: "checked", checked: true })
            }
            onKeyDown={(event) => handleKeyDown(event, field, "")}
          />
        );
      case "comboBox":
      case "listBox": {
        const selected = field.value ?? "";
        const listSize =
          field.kind === "listBox"
            ? Math.min(Math.max(field.options.length, 2), 5)
            : undefined;
        return (
          <select
            key={field.fieldId}
            ref={setControlRef(field.fieldId)}
            className={classNameFor(field, field.kind === "listBox" ? "list" : "combo")}
            aria-label={common["aria-label"]}
            title={common.title}
            style={rectStyle(field)}
            value={selected}
            size={listSize}
            disabled={!interactive(field)}
            onChange={(event) => {
              if (event.target.value) {
                void commit(field, {
                  kind: "choice",
                  value: event.target.value,
                });
              }
            }}
            onKeyDown={(event) => handleKeyDown(event, field, "")}
          >
            {selected === "" && (
              <option value="" disabled>
                请选择…
              </option>
            )}
            {selected !== "" && !field.options.includes(selected) && (
              <option value={selected}>{selected}</option>
            )}
            {field.options.map((option) => (
              <option key={option} value={option}>
                {option}
              </option>
            ))}
          </select>
        );
      }
      default:
        return null;
    }
  };

  return (
    <div
      ref={layerRef}
      className={`form-layer${placing ? " form-layer--placing" : ""}`}
      role="group"
      aria-label="表单字段"
      onPointerDown={handleStructurePointerDown}
      onPointerMove={handleStructurePointerMove}
      onPointerUp={handleStructurePointerUp}
      onPointerCancel={() => {
        setPlacementStart(null);
        setPlacementCurrent(null);
      }}
    >
      <div
        className="form-layer-toolbar"
        role="group"
        aria-label="表单工具"
        onPointerDown={(event) => event.stopPropagation()}
      >
        <button
          type="button"
          className="form-create-toggle"
          aria-pressed={placing}
          disabled={!canModifyContent || structureBusy}
          title={
            canModifyContent
              ? placing
                ? "退出表单字段工具"
                : "在页面上点击或拖动以放置文本字段"
              : STRUCTURE_DISABLED_REASON
          }
          onClick={togglePlacing}
        >
          添加文本字段
        </button>
        <span className="toolbar-divider" aria-hidden="true" />
        <button
          type="button"
          disabled={!history?.canUndo || structureBusy}
          title={history?.canUndo ? "撤销上一步编辑" : "没有可撤销的操作"}
          onClick={() => void runStructureHistory("undo")}
        >
          撤销
        </button>
        <button
          type="button"
          disabled={!history?.canRedo || structureBusy}
          title={history?.canRedo ? "重做上一步编辑" : "没有可重做的操作"}
          onClick={() => void runStructureHistory("redo")}
        >
          重做
        </button>
        {createdPageFields.length > 0 && (
          <button
            type="button"
            aria-expanded={selectedField !== null}
            disabled={structureBusy}
            onClick={openPropertyPanel}
          >
            字段属性
          </button>
        )}
        {!canModifyContent && (
          <span className="form-edit-reason">{STRUCTURE_DISABLED_REASON}</span>
        )}
      </div>

      {placing && placementStart && (
        <div
          className="form-field-create-preview"
          data-testid="form-field-create-preview"
          aria-hidden="true"
          style={(() => {
            const preview = placementRect(
              placementStart,
              placementCurrent,
              pageWidth,
              pageHeight,
            );
            return {
              left: `${(preview.x / pageWidth) * 100}%`,
              top: `${(preview.y / pageHeight) * 100}%`,
              width: `${(preview.width / pageWidth) * 100}%`,
              height: `${(preview.height / pageHeight) * 100}%`,
            };
          })()}
        />
      )}

      {selectedField && panelDraft && (
        <div
          className="form-field-panel"
          role="group"
          aria-label="表单字段属性"
          onPointerDown={(event) => event.stopPropagation()}
        >
          <div className="form-field-panel-heading">
            <strong>字段属性</strong>
            <button
              type="button"
              aria-label="关闭属性面板"
              onClick={() => setSelectedFieldId(null)}
            >
              关闭
            </button>
          </div>
          {createdPageFields.length > 1 && (
            <label className="form-field-panel-row">
              字段
              <select
                aria-label="选择要编辑的字段"
                value={selectedField.fieldId}
                disabled={structureBusy}
                onChange={(event) => selectField(event.target.value)}
              >
                {createdPageFields.map((field) => (
                  <option key={field.fieldId} value={field.fieldId}>
                    {field.name}
                  </option>
                ))}
              </select>
            </label>
          )}
          <label className="form-field-panel-row">
            名称
            <input
              type="text"
              aria-label="字段名称"
              value={panelDraft.name}
              disabled={structureBusy}
              spellCheck={false}
              onChange={(event) =>
                setPanelDraft({ ...panelDraft, name: event.target.value })
              }
              onBlur={() => void commitPanel()}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  void commitPanel();
                } else if (event.key === "Escape") {
                  event.preventDefault();
                  setPanelDraft(draftFor(selectedField));
                  setPanelError(null);
                }
              }}
            />
          </label>
          <label className="form-field-panel-row">
            提示文字
            <input
              type="text"
              aria-label="字段提示"
              value={panelDraft.tooltip}
              disabled={structureBusy}
              spellCheck={false}
              onChange={(event) =>
                setPanelDraft({ ...panelDraft, tooltip: event.target.value })
              }
              onBlur={() => void commitPanel()}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  void commitPanel();
                } else if (event.key === "Escape") {
                  event.preventDefault();
                  setPanelDraft(draftFor(selectedField));
                  setPanelError(null);
                }
              }}
            />
          </label>
          <label className="form-field-panel-row form-field-panel-row--check">
            <input
              type="checkbox"
              aria-label="必填字段"
              checked={panelDraft.required}
              disabled={structureBusy}
              onChange={(event) => {
                const next = {
                  ...panelDraft,
                  required: event.target.checked,
                };
                setPanelDraft(next);
                void commitPanel(next);
              }}
            />
            必填
          </label>
          <label className="form-field-panel-row form-field-panel-row--check">
            <input
              type="checkbox"
              aria-label="多行文本"
              checked={panelDraft.multiline}
              disabled={structureBusy}
              onChange={(event) => {
                const next = {
                  ...panelDraft,
                  multiline: event.target.checked,
                };
                setPanelDraft(next);
                void commitPanel(next);
              }}
            />
            多行
          </label>
          <label className="form-field-panel-row">
            字号
            <input
              type="number"
              aria-label="字号"
              min={4}
              max={200}
              step={1}
              value={panelDraft.fontSize}
              disabled={structureBusy}
              onChange={(event) =>
                setPanelDraft({ ...panelDraft, fontSize: event.target.value })
              }
              onBlur={() => void commitPanel()}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  void commitPanel();
                }
              }}
            />
          </label>
          <div className="form-field-panel-actions">
            <button
              type="button"
              className="form-field-delete"
              disabled={structureBusy}
              onClick={() => void deleteSelectedField()}
            >
              删除字段
            </button>
          </div>
          {panelError && (
            <span className="form-field-panel-error" role="alert">
              {panelError}
            </span>
          )}
        </div>
      )}

      {pageFields.map(renderControl)}
      {!canFillForms && (
        <span className="form-layer-notice" role="status">
          {FORMS_DISABLED_REASON}
        </span>
      )}
      {loadError && (
        <span
          className="form-layer-notice form-layer-notice--error"
          role="alert"
          title={loadError}
        >
          {LOAD_FAILED_REASON}
        </span>
      )}
      {notice && (
        <span className="form-layer-notice form-layer-notice--error" role="alert">
          {notice}
        </span>
      )}
    </div>
  );
}
