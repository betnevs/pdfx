import { useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";

import {
  deleteLink,
  updateLinkTarget,
  type LinkTarget,
  type LinkView,
} from "../lib/backend";
import { notifyDocumentChanged } from "../lib/documentEvents";
import {
  linkErrorText,
  linkTargetLabel,
  linkUnsupportedText,
} from "../lib/linkTarget";
import { LinkTargetDialog } from "./LinkTargetDialog";

const READ_ONLY_REASON = "此文档不允许编辑内容";
const ANNOTATION_PERMISSION_REASON = "此文档不允许添加注释";

type Notice = { kind: "info" | "error"; text: string };

export interface LinkLayerProps {
  documentId: string;
  pageIndex: number;
  /** Display-space page width, matching the text layer's coordinates. */
  pageWidth: number;
  pageHeight: number;
  /** Number of pages an internal link may address. */
  pageCount: number;
  /** Permission Restrictions forbid changing document content. */
  readOnly: boolean;
  /** The PDF security handler allows adding and modifying annotations. */
  canAnnotate: boolean;
  /** Every link in the document; this layer draws only its page's links. */
  links: LinkView[];
  /** Asks the owner to refetch after a mutation. */
  onLinksChanged: () => void;
}

function boxStyle(
  rect: {
    x: number;
    y: number;
    width: number;
    height: number;
  },
  pageWidth: number,
  pageHeight: number,
): CSSProperties {
  return {
    left: `${(rect.x / pageWidth) * 100}%`,
    top: `${(rect.y / pageHeight) * 100}%`,
    width: `${(rect.width / pageWidth) * 100}%`,
    height: `${(rect.height / pageHeight) * 100}%`,
  };
}

/**
 * Link editing overlay for one Page.
 *
 * Native links are invisible in the page bitmap, so the overlay only arms on
 * request: the 链接 toggle reveals one hit area per link quad, and a click
 * selects the link and outlines every quad it covers. The toolbar then shows
 * the target and offers edit/delete. This keeps text selection and the other
 * overlays usable while the link tool is off.
 *
 * A link written by another tool is read through its action or destination.
 * When PDFX cannot represent that target it refuses the edit with a Chinese
 * explanation and leaves the annotation untouched; the Rust command repeats
 * the same check before mutating.
 */
export function LinkLayer({
  documentId,
  pageIndex,
  pageWidth,
  pageHeight,
  pageCount,
  readOnly,
  canAnnotate,
  links,
  onLinksChanged,
}: LinkLayerProps) {
  const [active, setActive] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [dialogError, setDialogError] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);

  const editable = canAnnotate && !readOnly;
  const pageLinks = links.filter((link) => link.pageIndex === pageIndex);
  const selected =
    selectedId === null
      ? null
      : (pageLinks.find((link) => link.id === selectedId) ?? null);
  const unsupportedText = selected?.unsupported
    ? linkUnsupportedText(selected.unsupported)
    : null;

  if (!editable) {
    return (
      <div className="link-layer" role="group" aria-label="链接层" data-testid="link-layer">
        <div
          className="link-toolbar"
          role="toolbar"
          aria-label="链接工具"
          onPointerDown={(event) => event.stopPropagation()}
        >
          <button
            type="button"
            disabled
            title={readOnly ? READ_ONLY_REASON : ANNOTATION_PERMISSION_REASON}
          >
            链接
          </button>
          <span className="link-reason">
            {readOnly ? READ_ONLY_REASON : ANNOTATION_PERMISSION_REASON}
          </span>
        </div>
      </div>
    );
  }

  function toggle() {
    const next = !active;
    setActive(next);
    setNotice(null);
    if (!next) {
      setSelectedId(null);
      setDialogOpen(false);
      setDialogError(null);
    }
  }

  function select(link: LinkView) {
    setSelectedId(link.id);
    setNotice(null);
    setDialogError(null);
  }

  async function removeSelected() {
    if (!selected || !selected.target || busy) {
      return;
    }
    setBusy(true);
    setNotice(null);
    try {
      await deleteLink(documentId, selected.id);
      setSelectedId(null);
      setNotice({ kind: "info", text: "链接已删除" });
      onLinksChanged();
      notifyDocumentChanged({ documentId, pageIndex, source: "link" });
    } catch (error) {
      setNotice({ kind: "error", text: linkErrorText(error) });
    } finally {
      setBusy(false);
    }
  }

  async function submitTarget(target: LinkTarget) {
    if (!selected || busy) {
      return;
    }
    setBusy(true);
    setDialogError(null);
    setNotice(null);
    try {
      const result = await updateLinkTarget(documentId, {
        linkId: selected.id,
        target,
      });
      // Editing is remove-and-recreate on the Rust side, so the positional id
      // may change; select the canonical link the command returned.
      setSelectedId(result.link?.id ?? null);
      setDialogOpen(false);
      setNotice({ kind: "info", text: "链接已更新" });
      onLinksChanged();
      notifyDocumentChanged({ documentId, pageIndex, source: "link" });
    } catch (error) {
      setDialogError(linkErrorText(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      className={`link-layer${active ? " link-layer--active" : ""}`}
      role="group"
      aria-label="链接层"
      data-testid="link-layer"
    >
      <div
        className="link-toolbar"
        role="toolbar"
        aria-label="链接工具"
        onPointerDown={(event) => event.stopPropagation()}
      >
        <button
          type="button"
          aria-pressed={active}
          disabled={busy}
          title={
            active
              ? "退出链接选择"
              : "显示页面上的链接，点击可选择、编辑或删除"
          }
          onClick={toggle}
        >
          链接
        </button>
        {selected && (
          <>
            <span className="toolbar-divider" aria-hidden="true" />
            {selected.target && (
              <span className="link-target" data-testid="link-target">
                {linkTargetLabel(selected.target)}
              </span>
            )}
            {unsupportedText && (
              <span className="link-unsupported" role="status">
                {unsupportedText}
              </span>
            )}
            <button
              type="button"
              disabled={busy || selected.target === null}
              title={unsupportedText ?? "修改此链接的目标"}
              onClick={() => {
                setDialogError(null);
                setDialogOpen(true);
              }}
            >
              编辑链接
            </button>
            <button
              type="button"
              disabled={busy || selected.target === null}
              title={unsupportedText ?? "删除此链接（可撤销）"}
              onClick={() => void removeSelected()}
            >
              删除链接
            </button>
          </>
        )}
        {notice && (
          <span
            className={`link-notice link-notice--${notice.kind}`}
            role={notice.kind === "error" ? "alert" : "status"}
          >
            {notice.text}
          </span>
        )}
      </div>

      {active &&
        pageLinks.map((link) =>
          link.rects.map((rect, index) => (
            <button
              key={`${link.id}:${index}`}
              type="button"
              className="link-region"
              data-testid={`link-region-${link.id}-${index}`}
              aria-label={
                link.target
                  ? `选择链接：${linkTargetLabel(link.target)}`
                  : "选择链接"
              }
              style={boxStyle(rect, pageWidth, pageHeight)}
              onClick={() => select(link)}
            />
          )),
        )}

      {selected &&
        selected.rects.map((rect, index) => (
          <span
            key={index}
            className="link-selection"
            data-testid="link-selection"
            aria-hidden="true"
            style={boxStyle(rect, pageWidth, pageHeight)}
          />
        ))}

      {dialogOpen &&
        selected &&
        selected.target &&
        createPortal(
          <LinkTargetDialog
            mode="edit"
            pageCount={pageCount}
            initialTarget={selected.target}
            busy={busy}
            error={dialogError}
            onCancel={() => {
              if (!busy) {
                setDialogOpen(false);
                setDialogError(null);
              }
            }}
            onSubmit={(target) => void submitTarget(target)}
          />,
          document.body,
        )}
    </div>
  );
}
