import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";

import {
  fillFormField,
  formFields,
  toCommandError,
  type FormFieldView,
  type FormFieldValueInput,
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
const REQUIRED_MARK = "必填";

export interface FormLayerProps {
  documentId: string;
  pageIndex: number;
  /** Display-space page size in points, matching the rendered page box. */
  pageWidth: number;
  pageHeight: number;
  /** The document's Permission Restrictions allow changing form values. */
  canFillForms: boolean;
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
    reload();
    return () => {
      // Invalidate responses from the previous document.
      loadSequence.current += 1;
    };
  }, [documentId, reload]);

  // Undo, redo or another tool's edit can change field values; the tool that
  // made a form change already applied the command result, so its own event
  // only refreshes the other views.
  useEffect(
    () =>
      onDocumentChanged((detail) => {
        if (detail.documentId === documentId && detail.source !== "form") {
          reload();
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

  if (pageFields.length === 0 && !loadError && !notice) {
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
    <div className="form-layer" role="group" aria-label="表单字段">
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
