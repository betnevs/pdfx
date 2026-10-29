import { useEffect, useRef, useState, type FormEvent } from "react";

import type { BlankPageSize } from "../lib/backend";

/** Common paper sizes in PDF points (1 pt = 1/72 inch). */
export const A4_SIZE: BlankPageSize = { width: 595.28, height: 841.89 };
export const LETTER_SIZE: BlankPageSize = { width: 612, height: 792 };

type InsertSizeChoice = "current" | "a4" | "letter";

function formatSize(size: BlankPageSize): string {
  return `${Math.round(size.width)} × ${Math.round(size.height)} pt`;
}

/** Escape cancels a page dialog unless an operation is running. */
function useEscapeToCancel(cancel: () => void, busy: boolean) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) {
        event.preventDefault();
        cancel();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [busy, cancel]);
}

/**
 * Asks for the paper size of a blank page before inserting it.
 *
 * The default mirrors the page currently on screen so "insert a page like this
 * one" takes one click; A4 and Letter are the common alternatives.
 */
export function InsertBlankPageDialog({
  afterPageNumber,
  pageCount,
  currentPageSize,
  onInsert,
  onCancel,
}: {
  /** 1-based page number the new page will follow. */
  afterPageNumber: number;
  pageCount: number;
  /** Display size of the page the dialog was opened from, when known. */
  currentPageSize: BlankPageSize | null;
  onInsert: (size: BlankPageSize) => void;
  onCancel: () => void;
}) {
  const [choice, setChoice] = useState<InsertSizeChoice>(
    currentPageSize ? "current" : "a4",
  );
  const firstOptionRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    firstOptionRef.current?.focus();
  }, []);
  useEscapeToCancel(onCancel, false);

  const sizes: Array<{
    id: InsertSizeChoice;
    label: string;
    size: BlankPageSize | null;
  }> = [
    {
      id: "current",
      label: `与当前页相同（${currentPageSize ? formatSize(currentPageSize) : "等待页面信息"}）`,
      size: currentPageSize,
    },
    { id: "a4", label: `A4（${formatSize(A4_SIZE)}）`, size: A4_SIZE },
    { id: "letter", label: `Letter（${formatSize(LETTER_SIZE)}）`, size: LETTER_SIZE },
  ];

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const selected = sizes.find((option) => option.id === choice);
    if (selected?.size) {
      onInsert(selected.size);
    }
  };

  const position =
    afterPageNumber >= pageCount
      ? "文档末尾"
      : `第 ${afterPageNumber} 页之后`;

  return (
    <div className="password-overlay">
      <form
        className="password-dialog page-operation-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="insert-blank-page-title"
        onSubmit={submit}
      >
        <h2 id="insert-blank-page-title">插入空白页</h2>
        <p className="password-hint">
          新页面将插入到{position}。纸张尺寸：
        </p>
        <div className="page-dialog-options" role="radiogroup" aria-label="纸张尺寸">
          {sizes.map((option, index) => (
            <label key={option.id} className="page-dialog-option">
              <input
                ref={index === 0 ? firstOptionRef : undefined}
                type="radio"
                name="blank-page-size"
                value={option.id}
                checked={choice === option.id}
                disabled={option.size === null}
                onChange={() => setChoice(option.id)}
              />
              <span>{option.label}</span>
            </label>
          ))}
        </div>
        <div className="password-actions">
          <button type="button" className="secondary-button" onClick={onCancel}>
            取消
          </button>
          <button type="submit" className="open-button" disabled={!currentPageSize && choice === "current"}>
            插入
          </button>
        </div>
      </form>
    </div>
  );
}

/** Confirms deleting the current page; undo restores it with its content. */
export function DeletePageDialog({
  pageNumber,
  pageCount,
  onConfirm,
  onCancel,
}: {
  /** 1-based page number to delete. */
  pageNumber: number;
  pageCount: number;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    cancelRef.current?.focus();
  }, []);
  useEscapeToCancel(onCancel, false);

  return (
    <div className="password-overlay">
      <div
        className="password-dialog page-operation-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="delete-page-title"
      >
        <h2 id="delete-page-title">删除页面</h2>
        <p className="password-hint">
          确定删除第 {pageNumber} 页（共 {pageCount} 页）吗？删除后可以用撤销恢复该页及其注释和表单。
        </p>
        <div className="password-actions">
          <button
            ref={cancelRef}
            type="button"
            className="secondary-button"
            onClick={onCancel}
          >
            取消
          </button>
          <button
            type="button"
            className="page-dialog-danger"
            onClick={onConfirm}
          >
            删除页面
          </button>
        </div>
      </div>
    </div>
  );
}
