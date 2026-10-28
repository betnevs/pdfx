import {
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";

import {
  insertSignatureImage,
  toCommandError,
  type CommandErrorCode,
  type ImageObjectView,
} from "../lib/backend";
import {
  addStrokePoint,
  decodeUploadedImage,
  encodePng,
  ensureSignatureFont,
  hasTransparency,
  hasVisiblePixels,
  paintSignatureStrokes,
  processUploadedImage,
  renderDrawingPng,
  renderTypedPng,
  SIGNATURE_CANVAS_HEIGHT,
  SIGNATURE_CANVAS_WIDTH,
  SIGNATURE_LABELS,
  trimTransparent,
  uploadedFileError,
  validateSignatureName,
  whiteBackgroundRatio,
  type CanvasImage,
  type ProcessedUpload,
  type RgbaImage,
  type SignaturePoint,
  type SignatureStroke,
  type SignatureTab,
} from "../lib/signature";

/** Chinese copy for failures returned by the insert signature command. */
const SIGNATURE_ERROR_MESSAGES: Partial<Record<CommandErrorCode, string>> = {
  unsupported_image_format: "仅支持 PNG 和 JPG 格式的图片。",
  image_decode_failed: "无法读取这张图片，请确认文件是有效的 PNG 或 JPG。",
  image_too_large: "图片过大：文件不能超过 25 MB，像素不能超过 2000 万。",
  invalid_argument: "签名内容无效，请重试。",
  permission_denied: "文档所有者限制了编辑权限。",
  document_not_found: "文档已关闭，请重新打开。",
  page_out_of_range: "页码无效。",
};

function signatureErrorText(error: unknown): string {
  const commandError = toCommandError(error);
  return (
    SIGNATURE_ERROR_MESSAGES[commandError.code] ?? "签名插入失败，请重试。"
  );
}

interface UploadState {
  file: File;
  fileName: string;
  hadTransparency: boolean;
  removeBackground: boolean;
  preview: CanvasImage;
  trimmed: boolean;
  visible: boolean;
}

export interface SignatureDialogProps {
  documentId: string;
  pageIndex: number;
  onCancel: () => void;
  /** Called with the committed page object after the command succeeds. */
  onInserted: (image: ImageObjectView) => void;
}

/** Applies the upload tab's background choice, trimming transparent margins. */
function preparePixels(
  pixels: RgbaImage,
  removeBackground: boolean,
): ProcessedUpload {
  if (removeBackground) {
    return processUploadedImage(pixels, true);
  }
  const trimmed = trimTransparent(pixels);
  if (trimmed) {
    return { image: trimmed, removedBackground: false, trimmed: true };
  }
  return { image: pixels, removedBackground: false, trimmed: false };
}

/**
 * Chinese signature dialog with three entry points: draw, type a name, or
 * upload a PNG/JPEG.
 *
 * Every entry point produces the same thing — a transparent PNG in memory —
 * which the `insert_signature_image` command validates again and places as an
 * ordinary image page object. The payload never touches disk and is never
 * logged. Cancelling, Escape or a closed document leaves the document and its
 * edit history unchanged.
 */
export function SignatureDialog({
  documentId,
  pageIndex,
  onCancel,
  onInserted,
}: SignatureDialogProps) {
  const [tab, setTab] = useState<SignatureTab>("draw");
  const [strokes, setStrokes] = useState<SignatureStroke[]>([]);
  const [typedName, setTypedName] = useState("");
  const [typedPreview, setTypedPreview] = useState<CanvasImage | null>(null);
  const [upload, setUpload] = useState<UploadState | null>(null);
  const [uploadError, setUploadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const firstTabRef = useRef<HTMLButtonElement>(null);
  const drawing = useRef(false);

  useEffect(() => {
    firstTabRef.current?.focus();
  }, []);

  // Repaint the drawing canvas whenever a stroke changes or the tab returns.
  useEffect(() => {
    if (tab !== "draw") {
      return;
    }
    const ctx = canvasRef.current?.getContext("2d");
    if (!ctx) {
      return;
    }
    paintSignatureStrokes(
      ctx,
      strokes,
      SIGNATURE_CANVAS_WIDTH,
      SIGNATURE_CANVAS_HEIGHT,
    );
  }, [tab, strokes]);

  // Typed preview is rendered only for a valid name, after the bundled face
  // has loaded. The glyphs are rasterised; no font reaches the PDF.
  useEffect(() => {
    if (tab !== "type") {
      return;
    }
    const validation = validateSignatureName(typedName);
    if (validation) {
      setTypedPreview(null);
      return;
    }
    let cancelled = false;
    void ensureSignatureFont().then(() => {
      if (!cancelled) {
        setTypedPreview(renderTypedPng(typedName));
      }
    });
    return () => {
      cancelled = true;
    };
  }, [tab, typedName]);

  const typedValidation = validateSignatureName(typedName);
  const canInsert =
    !busy &&
    (tab === "draw"
      ? strokes.length > 0
      : tab === "type"
        ? typedValidation === null && typedPreview !== null
        : upload !== null && upload.visible);

  function pointFromEvent(
    event: ReactPointerEvent<HTMLCanvasElement>,
  ): SignaturePoint | null {
    const canvas = canvasRef.current;
    if (!canvas) {
      return null;
    }
    const box = canvas.getBoundingClientRect();
    if (box.width <= 0 || box.height <= 0) {
      return null;
    }
    return {
      x: ((event.clientX - box.left) / box.width) * SIGNATURE_CANVAS_WIDTH,
      y: ((event.clientY - box.top) / box.height) * SIGNATURE_CANVAS_HEIGHT,
    };
  }

  function beginStroke(event: ReactPointerEvent<HTMLCanvasElement>) {
    if (busy) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    event.currentTarget.setPointerCapture?.(event.pointerId);
    drawing.current = true;
    setError(null);
    setStrokes((current) => [...current, [point]]);
  }

  function extendStroke(event: ReactPointerEvent<HTMLCanvasElement>) {
    if (!drawing.current || busy) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    setStrokes((current) => {
      const last = current[current.length - 1];
      if (!last) {
        return current;
      }
      const extended = addStrokePoint(last, point);
      if (extended === last) {
        return current;
      }
      return [...current.slice(0, -1), extended];
    });
  }

  function endStroke(event: ReactPointerEvent<HTMLCanvasElement>) {
    drawing.current = false;
    try {
      event.currentTarget.releasePointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture may never have been acquired.
    }
  }

  async function acceptUpload(file: File) {
    setUploadError(null);
    setError(null);
    const rejection = await uploadedFileError(file);
    if (rejection) {
      setUpload(null);
      setUploadError(rejection);
      return;
    }
    let pixels: RgbaImage;
    try {
      pixels = await decodeUploadedImage(file);
    } catch {
      setUpload(null);
      setUploadError("无法读取这张图片，请确认文件是有效的 PNG 或 JPG。");
      return;
    }
    const hadTransparency = hasTransparency(pixels);
    const removeBackground =
      !hadTransparency && whiteBackgroundRatio(pixels) >= 0.5;
    const processed = preparePixels(pixels, removeBackground);
    const preview = encodePng(processed.image);
    if (!preview) {
      setUpload(null);
      setUploadError("无法读取这张图片，请确认文件是有效的 PNG 或 JPG。");
      return;
    }
    setUpload({
      file,
      fileName: file.name,
      hadTransparency,
      removeBackground,
      preview,
      trimmed: processed.trimmed,
      visible: hasVisiblePixels(processed.image),
    });
  }

  async function toggleBackgroundRemoval(next: boolean) {
    if (!upload || busy) {
      return;
    }
    setUploadError(null);
    let pixels: RgbaImage;
    try {
      pixels = await decodeUploadedImage(upload.file);
    } catch {
      setUploadError("无法读取这张图片，请确认文件是有效的 PNG 或 JPG。");
      return;
    }
    const processed = preparePixels(pixels, next);
    const preview = encodePng(processed.image);
    if (!preview) {
      return;
    }
    const visible = hasVisiblePixels(processed.image);
    setUpload({
      ...upload,
      removeBackground: next,
      preview,
      trimmed: processed.trimmed,
      visible,
    });
  }

  async function insert() {
    if (busy || !canInsert) {
      return;
    }
    setError(null);
    let image: CanvasImage | null;
    if (tab === "draw") {
      image = renderDrawingPng(strokes);
    } else if (tab === "type") {
      await ensureSignatureFont();
      image = renderTypedPng(typedName);
    } else {
      image = upload?.preview ?? null;
    }
    if (!image) {
      setError("签名内容为空，请重新书写或选择图片。");
      return;
    }
    setBusy(true);
    try {
      const result = await insertSignatureImage(
        documentId,
        pageIndex,
        image.base64,
        SIGNATURE_LABELS[tab],
      );
      onInserted(result.image);
    } catch (insertError) {
      setError(signatureErrorText(insertError));
    } finally {
      setBusy(false);
    }
  }

  const tabHint =
    tab === "draw"
      ? "在下方画布上书写签名，可用“撤销”和“清除”修改。"
      : tab === "type"
        ? typedValidation ?? "姓名将以手写字体生成透明签名。"
        : uploadError ??
          (!upload
            ? "选择一张 PNG 或 JPG 图片作为签名。"
            : !upload.visible
              ? "图片中没有可见内容，请选择其他图片。"
              : upload.hadTransparency
                ? `已保留 PNG 的透明背景。${upload.trimmed ? "已裁掉透明边距。" : ""}`
                : upload.removeBackground
                  ? `已移除白色背景${upload.trimmed ? "并裁掉透明边距" : ""}。`
                  : "图片背景不透明，签名将保留原始背景。可勾选“移除白色背景”。");

  return (
    <div
      className="signature-dialog-backdrop"
      onPointerDown={(event) => event.stopPropagation()}
    >
      <form
        className="signature-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="signature-dialog-title"
        onSubmit={(event) => {
          event.preventDefault();
          void insert();
        }}
        onKeyDown={(event) => {
          if (event.key === "Escape" && !busy) {
            event.preventDefault();
            onCancel();
          }
        }}
      >
        <h2 id="signature-dialog-title">插入签名</h2>

        <div
          className="signature-dialog-tabs"
          role="tablist"
          aria-label="签名方式"
        >
          {(["draw", "type", "upload"] as const).map((entry, index) => (
            <button
              key={entry}
              ref={index === 0 ? firstTabRef : undefined}
              type="button"
              role="tab"
              id={`signature-tab-${entry}`}
              aria-selected={tab === entry}
              aria-controls={`signature-panel-${entry}`}
              disabled={busy}
              onClick={() => {
                setTab(entry);
                setError(null);
              }}
            >
              {TAB_LABELS[entry]}
            </button>
          ))}
        </div>

        {tab === "draw" && (
          <div
            role="tabpanel"
            id="signature-panel-draw"
            aria-labelledby="signature-tab-draw"
          >
            <canvas
              ref={canvasRef}
              className="signature-dialog-canvas"
              data-testid="signature-canvas"
              aria-label="签名画布"
              width={SIGNATURE_CANVAS_WIDTH}
              height={SIGNATURE_CANVAS_HEIGHT}
              onPointerDown={beginStroke}
              onPointerMove={extendStroke}
              onPointerUp={endStroke}
              onPointerCancel={endStroke}
            />
            <div className="signature-dialog-actions">
              <button
                type="button"
                disabled={busy || strokes.length === 0}
                onClick={() => setStrokes((current) => current.slice(0, -1))}
              >
                撤销
              </button>
              <button
                type="button"
                disabled={busy || strokes.length === 0}
                onClick={() => setStrokes([])}
              >
                清除
              </button>
            </div>
          </div>
        )}

        {tab === "type" && (
          <div
            role="tabpanel"
            id="signature-panel-type"
            aria-labelledby="signature-tab-type"
          >
            <label className="signature-dialog-field">
              姓名
              <input
                type="text"
                value={typedName}
                disabled={busy}
                maxLength={200}
                placeholder="请输入姓名"
                onChange={(event) => setTypedName(event.target.value)}
              />
            </label>
            {typedPreview && (
              <img
                className="signature-dialog-preview"
                src={typedPreview.dataUrl}
                alt="签名预览"
              />
            )}
          </div>
        )}

        {tab === "upload" && (
          <div
            role="tabpanel"
            id="signature-panel-upload"
            aria-labelledby="signature-tab-upload"
          >
            <label className="signature-dialog-field">
              选择图片
              <input
                type="file"
                accept="image/png,image/jpeg"
                aria-label="选择签名图片"
                disabled={busy}
                onChange={(event) => {
                  const file = event.target.files?.[0];
                  if (file) {
                    void acceptUpload(file);
                  }
                  // Allow choosing the same file again after a failed attempt.
                  event.target.value = "";
                }}
              />
            </label>
            {upload && !upload.hadTransparency && (
              <label className="signature-dialog-toggle">
                <input
                  type="checkbox"
                  checked={upload.removeBackground}
                  disabled={busy}
                  onChange={(event) =>
                    void toggleBackgroundRemoval(event.target.checked)
                  }
                />
                移除白色背景
              </label>
            )}
            {upload && (
              <>
                <img
                  className={`signature-dialog-preview${upload.visible ? "" : " signature-dialog-preview--empty"}`}
                  src={upload.preview.dataUrl}
                  alt="签名预览"
                />
                <p className="signature-dialog-filename">{upload.fileName}</p>
              </>
            )}
          </div>
        )}

        <p className="signature-dialog-hint" role="status">
          {tabHint}
        </p>
        {error && (
          <p className="signature-dialog-error" role="alert">
            {error}
          </p>
        )}

        <div className="signature-dialog-actions">
          <button type="button" disabled={busy} onClick={onCancel}>
            取消
          </button>
          <button type="submit" disabled={!canInsert}>
            {busy ? "正在插入…" : "插入签名"}
          </button>
        </div>
      </form>
    </div>
  );
}

const TAB_LABELS: Record<SignatureTab, string> = {
  draw: "绘制",
  type: "输入",
  upload: "上传",
};
