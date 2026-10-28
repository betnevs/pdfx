import { useEffect, useRef, useState } from "react";

import {
  applyWatermark,
  chooseImageFile,
  deleteWatermark,
  toCommandError,
  watermarkImagePreview,
  watermarkState,
  type CommandErrorCode,
  type EditHistoryView,
  type WatermarkImagePreview,
  type WatermarkKind,
  type WatermarkScope,
  type WatermarkView,
} from "../lib/backend";
import { onDocumentChanged } from "../lib/documentEvents";

const MAX_TEXT_CHARS = 200;

const WATERMARK_ERROR_MESSAGES: Partial<Record<CommandErrorCode, string>> = {
  file_not_found: "找不到图片文件，它可能已被移动或删除。",
  file_read_failed: "无法读取该图片文件。",
  permission_denied: "当前文档的权限不允许修改水印。",
  read_only_document: "文档为只读，无法修改水印。",
  document_not_found: "文档已关闭。",
  unsupported_image_format: "请选择 PNG 或 JPEG 图片。",
  image_decode_failed: "无法解码该图片，请换一张 PNG 或 JPEG 图片。",
  image_too_large: "图片过大，无法用作水印。",
  invalid_argument: "水印设置无效，请检查后重试。",
};

/** Chinese copy for a watermark command failure. */
export function watermarkErrorMessage(code: CommandErrorCode): string {
  return WATERMARK_ERROR_MESSAGES[code] ?? "水印操作失败，请重试。";
}

export interface WatermarkPanelProps {
  documentId: string;
  /** Current logical page, used by the "current page" scope. */
  pageIndex: number;
  /** Permission Restrictions deny every editing capability. */
  readOnly: boolean;
  /** Permission Restrictions allow changing page content. */
  canModifyContent: boolean;
  /** Feeds the unified history so the dirty marker and menu update. */
  onHistory: (history: EditHistoryView) => void;
  /**
   * Announces which thumbnails are stale: the current page only, or every
   * page after an all-pages apply, a delete, or an undo/redo.
   */
  onApplied: (allPages: boolean) => void;
}

/**
 * Watermark controls for the current Page or every visible logical Page.
 *
 * A watermark is page content, not an annotation. The panel holds one active
 * configuration per editing session; applying again replaces it, and delete,
 * undo and redo remain one unified history action each. The preview box
 * updates locally on every slider change so the user does not need to apply a
 * mark to see where it lands.
 */
export function WatermarkPanel({
  documentId,
  pageIndex,
  readOnly,
  canModifyContent,
  onHistory,
  onApplied,
}: WatermarkPanelProps) {
  const editable = canModifyContent && !readOnly;
  const [mode, setMode] = useState<WatermarkKind>("text");
  const [text, setText] = useState("");
  const [imagePath, setImagePath] = useState<string | null>(null);
  const [preview, setPreview] = useState<WatermarkImagePreview | null>(null);
  const [opacity, setOpacity] = useState(35);
  const [rotation, setRotation] = useState(0);
  const [scale, setScale] = useState(100);
  const [positionX, setPositionX] = useState(50);
  const [positionY, setPositionY] = useState(50);
  const [scope, setScope] = useState<WatermarkScope>("current");
  const [active, setActive] = useState<WatermarkView | null>(null);
  const [busy, setBusy] = useState<
    "load" | "preview" | "apply" | "delete" | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const generationRef = useRef(0);
  const aliveRef = useRef(true);

  // The active watermark is session state on the Rust side. Re-read it on
  // mount, after this panel's own changes, after we are told the change came
  // from the watermark tool, and after undo/redo (which App reports as an
  // all-pages invalidation). Unrelated content edits must not reset the form.
  useEffect(() => {
    aliveRef.current = true;

    async function load() {
      const current = ++generationRef.current;
      try {
        const view = await watermarkState(documentId);
        if (!aliveRef.current || current !== generationRef.current) {
          return;
        }
        hydrate(view);
        setActive(view);
        setBusy((value) => (value === "load" ? null : value));
      } catch (failure) {
        if (!aliveRef.current || current !== generationRef.current) {
          return;
        }
        setError(watermarkErrorMessage(toCommandError(failure).code));
        setBusy((value) => (value === "load" ? null : value));
      }
    }

    function hydrate(view: WatermarkView | null) {
      if (!view) {
        return;
      }
      setMode(view.kind);
      setScope(view.scope);
      setOpacity(view.opacity);
      setRotation(view.rotation);
      setScale(view.scale);
      setPositionX(Math.round(view.positionX * 100));
      setPositionY(Math.round(view.positionY * 100));
      if (view.kind === "text") {
        setText(view.text ?? "");
        return;
      }
      setImagePath(view.imagePath);
      if (view.imagePath) {
        void loadPreview(view.imagePath);
      }
    }

    async function loadPreview(path: string) {
      const current = ++generationRef.current;
      try {
        const next = await watermarkImagePreview(path);
        if (aliveRef.current && current === generationRef.current) {
          setPreview(next);
        }
      } catch (failure) {
        if (aliveRef.current && current === generationRef.current) {
          setError(watermarkErrorMessage(toCommandError(failure).code));
        }
      }
    }

    void load();
    return onDocumentChanged((detail) => {
      if (
        detail.documentId === documentId &&
        (detail.allPagesChanged ||
          detail.pageStructureChanged ||
          detail.source === "watermark")
      ) {
        void load();
      }
    });
  }, [documentId]);

  useEffect(
    () => () => {
      aliveRef.current = false;
    },
    [],
  );

  async function pickImage() {
    if (!editable || busy) {
      return;
    }
    setError(null);
    setNotice(null);
    let path: string | null;
    try {
      path = await chooseImageFile();
    } catch (failure) {
      setError(watermarkErrorMessage(toCommandError(failure).code));
      return;
    }
    if (!path) {
      return;
    }
    setBusy("preview");
    try {
      const next = await watermarkImagePreview(path);
      setImagePath(path);
      setPreview(next);
    } catch (failure) {
      setError(watermarkErrorMessage(toCommandError(failure).code));
    } finally {
      setBusy(null);
    }
  }

  async function apply() {
    if (!editable || busy) {
      return;
    }
    setError(null);
    setNotice(null);
    const trimmed = text.trim();
    if (mode === "text") {
      if (!trimmed) {
        setError("请输入水印文字。");
        return;
      }
      if (/[\r\n]/.test(trimmed)) {
        setError("水印文字只能输入一行。");
        return;
      }
      if (Array.from(trimmed).length > MAX_TEXT_CHARS) {
        setError(`水印文字不能超过 ${MAX_TEXT_CHARS} 个字符。`);
        return;
      }
    } else if (!imagePath) {
      setError("请先选择水印图片。");
      return;
    }

    setBusy("apply");
    try {
      const result = await applyWatermark(documentId, pageIndex, {
        scope,
        kind: mode,
        text: mode === "text" ? trimmed : null,
        imagePath: mode === "image" ? imagePath : null,
        opacity,
        rotation,
        scale,
        positionX: positionX / 100,
        positionY: positionY / 100,
      });
      setActive(result.watermark);
      onHistory(result.history);
      onApplied(scope === "all");
      setNotice(
        scope === "all" ? "已应用到所有可见页面。" : "已应用到当前页。",
      );
    } catch (failure) {
      setError(watermarkErrorMessage(toCommandError(failure).code));
    } finally {
      setBusy(null);
    }
  }

  async function remove() {
    if (!editable || busy || !active) {
      return;
    }
    setError(null);
    setNotice(null);
    setBusy("delete");
    try {
      const result = await deleteWatermark(documentId);
      setActive(null);
      onHistory(result.history);
      // A delete may remove a mark from every page, so invalidate them all;
      // this is also correct for the single-page case.
      onApplied(true);
      setNotice("已删除水印。");
    } catch (failure) {
      setError(watermarkErrorMessage(toCommandError(failure).code));
    } finally {
      setBusy(null);
    }
  }

  const disabled = !editable || busy !== null;
  const previewText = text.trim() || "水印文字";

  return (
    <aside className="watermark-panel" aria-label="水印面板">
      <header className="watermark-panel-header">
        <h2>水印</h2>
        {active && (
          <span className="watermark-active-count">
            {active.pageIndices.length} 页
          </span>
        )}
      </header>

      {!editable && (
        <p className="watermark-readonly" role="note">
          {readOnly
            ? "文档为只读，无法修改水印。"
            : "当前文档的权限不允许修改水印。"}
        </p>
      )}

      {error && (
        <p className="watermark-error" role="alert">
          {error}
        </p>
      )}
      {notice && (
        <p className="watermark-notice" role="status">
          {notice}
        </p>
      )}
      {active && (
        <p className="watermark-active" role="status">
          当前水印：
          {active.kind === "text"
            ? `文字「${active.text ?? ""}」`
            : `图片 ${active.imageName ?? ""}`}
          （{active.pageIndices.length} 页）
        </p>
      )}

      <div className="watermark-preview" aria-label="水印预览">
        <span
          className={`watermark-preview-mark watermark-preview-mark--${mode}`}
          style={{
            left: `${positionX}%`,
            top: `${positionY}%`,
            opacity: opacity / 100,
            transform: `translate(-50%, -50%) rotate(${rotation}deg) scale(${scale / 100})`,
          }}
        >
          {mode === "text" ? (
            <span className="watermark-preview-text">{previewText}</span>
          ) : preview ? (
            <img
              className="watermark-preview-image"
              src={preview.dataUrl}
              alt=""
              draggable={false}
            />
          ) : (
            <span className="watermark-preview-empty">未选择图片</span>
          )}
        </span>
      </div>

      <div className="watermark-modes" role="radiogroup" aria-label="水印类型">
        <label>
          <input
            type="radio"
            name="watermark-kind"
            checked={mode === "text"}
            disabled={disabled}
            onChange={() => {
              setMode("text");
              setError(null);
            }}
          />
          文字
        </label>
        <label>
          <input
            type="radio"
            name="watermark-kind"
            checked={mode === "image"}
            disabled={disabled}
            onChange={() => {
              setMode("image");
              setError(null);
            }}
          />
          图片
        </label>
      </div>

      {mode === "text" ? (
        <label className="watermark-field">
          <span>水印文字</span>
          <input
            type="text"
            value={text}
            maxLength={MAX_TEXT_CHARS}
            placeholder="输入水印文字"
            disabled={disabled}
            onChange={(event) => {
              setText(event.target.value);
              setError(null);
            }}
          />
        </label>
      ) : (
        <div className="watermark-image-field">
          <button
            type="button"
            className="watermark-choose"
            disabled={disabled}
            onClick={() => void pickImage()}
          >
            {busy === "preview" ? "正在读取图片…" : "选择图片…"}
          </button>
          <span className="watermark-image-name" title={imagePath ?? undefined}>
            {preview
              ? `${preview.name} · ${preview.width}×${preview.height} px`
              : "支持 PNG、JPEG"}
          </span>
        </div>
      )}

      <label className="watermark-field">
        <span className="watermark-field-label">
          不透明度 <span className="watermark-value">{opacity}%</span>
        </span>
        <input
          type="range"
          min={1}
          max={100}
          step={1}
          value={opacity}
          disabled={disabled}
          aria-label="不透明度"
          onChange={(event) => setOpacity(Number(event.target.value))}
        />
      </label>
      <label className="watermark-field">
        <span className="watermark-field-label">
          旋转 <span className="watermark-value">{rotation}°</span>
        </span>
        <input
          type="range"
          min={-180}
          max={180}
          step={1}
          value={rotation}
          disabled={disabled}
          aria-label="旋转"
          onChange={(event) => setRotation(Number(event.target.value))}
        />
      </label>
      <label className="watermark-field">
        <span className="watermark-field-label">
          缩放 <span className="watermark-value">{scale}%</span>
        </span>
        <input
          type="range"
          min={10}
          max={400}
          step={5}
          value={scale}
          disabled={disabled}
          aria-label="缩放"
          onChange={(event) => setScale(Number(event.target.value))}
        />
      </label>
      <label className="watermark-field">
        <span className="watermark-field-label">
          水平位置 <span className="watermark-value">{positionX}%</span>
        </span>
        <input
          type="range"
          min={0}
          max={100}
          step={1}
          value={positionX}
          disabled={disabled}
          aria-label="水平位置"
          onChange={(event) => setPositionX(Number(event.target.value))}
        />
      </label>
      <label className="watermark-field">
        <span className="watermark-field-label">
          垂直位置 <span className="watermark-value">{positionY}%</span>
        </span>
        <input
          type="range"
          min={0}
          max={100}
          step={1}
          value={positionY}
          disabled={disabled}
          aria-label="垂直位置"
          onChange={(event) => setPositionY(Number(event.target.value))}
        />
      </label>

      <div className="watermark-scopes" role="radiogroup" aria-label="水印范围">
        <label>
          <input
            type="radio"
            name="watermark-scope"
            checked={scope === "current"}
            disabled={disabled}
            onChange={() => setScope("current")}
          />
          当前页
        </label>
        <label>
          <input
            type="radio"
            name="watermark-scope"
            checked={scope === "all"}
            disabled={disabled}
            onChange={() => setScope("all")}
          />
          所有页面
        </label>
      </div>

      <div className="watermark-actions">
        <button
          type="button"
          className="watermark-apply"
          disabled={disabled}
          onClick={() => void apply()}
        >
          {busy === "apply" ? "正在应用…" : "应用水印"}
        </button>
        <button
          type="button"
          className="watermark-delete"
          disabled={disabled || !active}
          onClick={() => void remove()}
        >
          {busy === "delete" ? "正在删除…" : "删除水印"}
        </button>
      </div>
    </aside>
  );
}
