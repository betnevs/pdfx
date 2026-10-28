import { useEffect, useRef, useState } from "react";

import type { LinkTarget } from "../lib/backend";
import { validateLinkPage, validateLinkUrl } from "../lib/linkTarget";

export interface LinkTargetDialogProps {
  /** `create` adds a link over a selection; `edit` replaces a link's target. */
  mode: "create" | "edit";
  /** Number of pages the one-based page field may address. */
  pageCount: number;
  /** The current target in edit mode; null when creating a link. */
  initialTarget: LinkTarget | null;
  busy: boolean;
  /** Backend failure shown inside the dialog, already translated. */
  error: string | null;
  onCancel: () => void;
  onSubmit: (target: LinkTarget) => void;
}

/**
 * Chinese URL/Page dialog for creating or editing a link target.
 *
 * Validation runs before the command so a typo never reaches the document:
 * URLs must use `http://`, `https://` or `mailto:`, and the page number is
 * one-based and checked against the document's page count. The same rules are
 * enforced again by the Rust command boundary.
 */
export function LinkTargetDialog({
  mode,
  pageCount,
  initialTarget,
  busy,
  error,
  onCancel,
  onSubmit,
}: LinkTargetDialogProps) {
  const [kind, setKind] = useState<LinkTarget["kind"]>(
    initialTarget?.kind ?? "url",
  );
  const [url, setUrl] = useState(
    initialTarget?.kind === "url" ? initialTarget.url : "",
  );
  const [page, setPage] = useState(
    initialTarget?.kind === "page" ? String(initialTarget.pageIndex + 1) : "",
  );
  const [localError, setLocalError] = useState<string | null>(null);
  const urlRef = useRef<HTMLInputElement>(null);
  const pageRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (kind === "url") {
      urlRef.current?.focus();
    } else {
      pageRef.current?.focus();
    }
  }, [kind]);

  const message = localError ?? error;
  const title = mode === "create" ? "添加链接" : "编辑链接";

  function submit() {
    if (busy) {
      return;
    }
    const validation =
      kind === "url" ? validateLinkUrl(url) : validateLinkPage(page, pageCount);
    if (!validation.ok) {
      setLocalError(validation.error);
      return;
    }
    setLocalError(null);
    onSubmit(validation.target);
  }

  return (
    <div
      className="link-dialog-backdrop"
      onPointerDown={(event) => event.stopPropagation()}
    >
      <form
        className="link-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="link-dialog-title"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
        onKeyDown={(event) => {
          if (event.key === "Escape" && !busy) {
            event.preventDefault();
            onCancel();
          }
        }}
      >
        <h2 id="link-dialog-title">{title}</h2>
        <fieldset className="link-dialog-kinds" disabled={busy}>
          <legend>链接类型</legend>
          <label>
            <input
              type="radio"
              name="link-target-kind"
              checked={kind === "url"}
              onChange={() => {
                setKind("url");
                setLocalError(null);
              }}
            />
            网址
          </label>
          <label>
            <input
              type="radio"
              name="link-target-kind"
              checked={kind === "page"}
              onChange={() => {
                setKind("page");
                setLocalError(null);
              }}
            />
            页面
          </label>
        </fieldset>

        {kind === "url" ? (
          <label className="link-dialog-field">
            链接网址
            <input
              ref={urlRef}
              type="text"
              aria-label="链接网址"
              value={url}
              disabled={busy}
              placeholder="https://example.com"
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => {
                setUrl(event.target.value);
                setLocalError(null);
              }}
            />
          </label>
        ) : (
          <label className="link-dialog-field">
            目标页码
            <input
              ref={pageRef}
              type="text"
              inputMode="numeric"
              aria-label="目标页码"
              value={page}
              disabled={busy}
              placeholder="1"
              autoComplete="off"
              onChange={(event) => {
                setPage(event.target.value);
                setLocalError(null);
              }}
            />
            <span className="link-dialog-hint">第 1 至 {pageCount} 页</span>
          </label>
        )}

        {message && (
          <p className="link-dialog-error" role="alert">
            {message}
          </p>
        )}

        <div className="link-dialog-actions">
          <button type="button" className="secondary-button" disabled={busy} onClick={onCancel}>
            取消
          </button>
          <button type="submit" className="open-button" disabled={busy}>
            {mode === "create" ? "添加" : "保存"}
          </button>
        </div>
      </form>
    </div>
  );
}
