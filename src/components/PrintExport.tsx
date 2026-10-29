import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from "react";
import { createPortal, flushSync } from "react-dom";

import {
  choosePageImageDestination,
  chooseSaveFile,
  exportFlattenedCopy,
  exportPage,
  pageInfo,
  printDocument,
  renderPage,
  toCommandError,
  type CommandError,
  type DocumentView,
  type FlattenExportView,
  type PageImageFormat,
} from "../lib/backend";
import { documentPolicy } from "../lib/document-policy";

export interface PrintExportHandle {
  openPrintDialog: () => void;
  openExportDialog: () => void;
}

interface PrintExportProps {
  document: DocumentView;
  pageIndex: number;
}

type Dialog = "print" | "export" | "flatten";
type PrintRange = "all" | "current" | "custom";
type PrintOrientation = "auto" | "portrait" | "landscape";
type PrintScale = "fit" | "actual";

interface PrintPage {
  pageIndex: number;
  /**
   * Object URL for the page's compressed PNG Blob. The print surface never
   * embeds base64, and the Blob is released when the print job ends.
   */
  imageUrl: string;
  /** Display size in points (CSS pixels at 72 DPI). */
  width: number;
  height: number;
}

interface PrintSize {
  width: number;
  height: number;
}

/** Print bitmaps are rendered at 150 DPI, twice the screen's point size. */
const PRINT_DPI = 150;

/** Export resolutions offered in the dialog, in DPI. */
const EXPORT_RESOLUTIONS = [72, 150, 300] as const;
const DEFAULT_EXPORT_RESOLUTION = 150;

const EXPORT_ERROR_MESSAGES: Partial<Record<CommandError["code"], string>> = {
  permission_denied: "没有写入该位置的权限，请选择其他位置。",
  file_write_failed: "无法写入文件，请检查目标位置与磁盘空间。",
  page_out_of_range: "页面不存在，文档可能已关闭。",
  export_target_is_source: "不能覆盖源 PDF 文件，请选择其他位置。",
  export_target_is_output: "不能覆盖当前输出 PDF 文件，请选择其他位置。",
  invalid_argument: "导出参数无效，请检查后重试。",
};

const FLATTEN_ERROR_MESSAGES: Partial<Record<CommandError["code"], string>> = {
  permission_denied: "没有写入该位置的权限，请选择其他位置。",
  file_write_failed: "无法写入文件，请检查目标位置与磁盘空间。",
  export_target_is_source: "不能覆盖源 PDF 文件，请选择其他位置。",
  export_target_is_output: "不能覆盖当前输出 PDF 文件，请选择其他位置。",
  unencrypted_output_confirmation_required:
    "加密文档需要确认后才能导出未加密的 Flatten 副本。",
  document_not_found: "文档已关闭，无法导出 Flatten 副本。",
  invalid_argument: "导出参数无效，请检查后重试。",
  flatten_export_failed: "无法生成 Flatten 副本，请检查文档后重试。",
};

const PRINT_ERROR_MESSAGES: Partial<Record<CommandError["code"], string>> = {
  permission_denied: "文档所有者限制了打印权限。",
  print_failed: "无法打开打印对话框。",
  render_failed: "无法渲染页面，打印已取消。",
  document_not_found: "文档已关闭，无法打印。",
};

function describeError(
  error: CommandError,
  messages: Partial<Record<CommandError["code"], string>>,
  fallback: string,
): string {
  return messages[error.code] ?? `${fallback}：${error.message}`;
}

/** Default file name offered by the save dialog, without a directory. */
function suggestedFileName(
  documentName: string,
  pageNumber: number,
  format: PageImageFormat,
): string {
  const base = documentName.replace(/\.pdf$/i, "") || "document";
  return `${base}-page-${pageNumber}.${format}`;
}

/** Default name for a flattened copy; it is always a new file. */
function suggestedFlattenedFileName(documentName: string): string {
  const base = documentName.replace(/\.pdf$/i, "") || "document";
  return `${base}-flattened.pdf`;
}

/** Chinese success summary for a written flattened copy. */
function flattenNotice(result: FlattenExportView): string {
  const detail = `flatten ${result.flattened} 个注释，保留 ${result.preserved} 个交互项`;
  const skipped =
    result.skipped.length > 0
      ? `，跳过 ${result.skipped.length} 个无页面外观的注释`
      : "";
  return `已导出 Flatten 副本：${result.path}（${detail}${skipped}）`;
}

function printPageBoxSize(
  page: PrintPage,
  orientation: PrintOrientation,
): PrintSize {
  const short = Math.min(page.width, page.height);
  const long = Math.max(page.width, page.height);
  if (orientation === "portrait") {
    return { width: short, height: long };
  }
  if (orientation === "landscape") {
    return { width: long, height: short };
  }
  return { width: page.width, height: page.height };
}
/**
 * Converts one rendered page into a Blob object URL. The base64 payload is
 * released as soon as the Blob exists, so the print surface never embeds it.
 */
function printBitmapUrl(pngBase64: string): string {
  const binary = atob(pngBase64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
}

/**
 * Decodes one prepared print bitmap before the next page is prepared, so the
 * print job never has every page's decoded RGBA in memory at once. Browsers
 * without `HTMLImageElement.decode` (the jsdom test environment) skip this
 * step. A decode failure keeps the page in the surface: the platform can still
 * render it from the Blob when it reaches that page.
 */
async function decodePrintImage(imageUrl: string): Promise<void> {
  if (typeof Image === "undefined") {
    return;
  }
  const image = new Image();
  if (typeof image.decode !== "function") {
    return;
  }
  image.src = imageUrl;
  try {
    await image.decode();
  } catch {
    // Printing proceeds; a page that cannot be decoded here may still print.
  }
}

/**
 * Renders one page at print resolution, decodes it and releases its base64
 * payload. Returns null when the job was cancelled, in which case any object
 * URL it created has already been revoked.
 */
async function preparePrintPage(
  documentId: string,
  pageIndex: number,
  isCancelled: () => boolean,
): Promise<PrintPage | null> {
  const [image, info] = await Promise.all([
    renderPage(documentId, pageIndex, PRINT_DPI / 72),
    pageInfo(documentId, pageIndex),
  ]);
  if (isCancelled()) {
    return null;
  }
  const imageUrl = printBitmapUrl(image.pngBase64);
  if (isCancelled()) {
    URL.revokeObjectURL(imageUrl);
    return null;
  }
  await decodePrintImage(imageUrl);
  if (isCancelled()) {
    URL.revokeObjectURL(imageUrl);
    return null;
  }
  return {
    pageIndex,
    imageUrl,
    width: info.width,
    height: info.height,
  };
}

/**
 * Print, page-export and flattened-copy actions for the open Document.
 *
 * All three are read-only for the open Document: they never change it or its
 * unsaved state. Export is always available; printing is disabled when the
 * Document's Permission Restrictions deny it.
 *
 * Flattened-copy export serializes the live state (including unsaved edits)
 * into a new file whose visual annotations are page content while links and
 * form widgets stay interactive. An encrypted Source Document requires an
 * explicit unencrypted-output confirmation first (ADR-0007).
 *
 * Printing renders the selected Pages into a hidden print-only surface (one
 * PNG Blob per Page, a `@page` box in points and a page break between Pages).
 * The platform dialog therefore offers the correct page range, orientation
 * and scaling options. Pages are prepared and decoded strictly one at a time:
 * the print DOM holds object URLs, never base64, and at most one page's
 * decoded RGBA bitmap exists during preparation, so decoded pixel memory is
 * one page regardless of whether the job has 2 or 2000 Pages. Preparation
 * reports progress and can be cancelled, which releases every prepared Blob
 * and the print surface.
 */
export const PrintExport = forwardRef<PrintExportHandle, PrintExportProps>(
  function PrintExport({ document, pageIndex }, ref) {
    const policy = documentPolicy(document);
    const [dialog, setDialog] = useState<Dialog | null>(null);
    const [notice, setNotice] = useState<string | null>(null);
    const [dialogError, setDialogError] = useState<string | null>(null);

    const [format, setFormat] = useState<PageImageFormat>("png");
    const [dpi, setDpi] = useState<number>(DEFAULT_EXPORT_RESOLUTION);
    const [exporting, setExporting] = useState(false);
    const [flattening, setFlattening] = useState(false);
    const [confirmFlattenUnencrypted, setConfirmFlattenUnencrypted] =
      useState(false);

    const [printRange, setPrintRange] = useState<PrintRange>("all");
    const [customFrom, setCustomFrom] = useState("1");
    const [customTo, setCustomTo] = useState(String(document.pageCount));
    const [orientation, setOrientation] = useState<PrintOrientation>("auto");
    const [printScale, setPrintScale] = useState<PrintScale>("fit");
    const [progress, setProgress] = useState<{
      done: number;
      total: number;
    } | null>(null);
    const [printPages, setPrintPages] = useState<PrintPage[] | null>(null);
    const [printSize, setPrintSize] = useState<PrintSize | null>(null);
    const cancelPrintRef = useRef(false);
    /** Invalidates a preparation loop when the user cancels or starts over. */
    const preparationGenerationRef = useRef(0);
    /** Object URLs alive in the print surface or still being prepared. */
    const livePrintUrlsRef = useRef<Set<string>>(new Set());
    const preparing = progress !== null;

    function releaseAllPrintUrls() {
      for (const url of livePrintUrlsRef.current) {
        URL.revokeObjectURL(url);
      }
      livePrintUrlsRef.current.clear();
    }

    // A new Document invalidates everything prepared for the previous one.
    useEffect(() => {
      cancelPrintRef.current = true;
      preparationGenerationRef.current += 1;
      releaseAllPrintUrls();
      setDialog(null);
      setNotice(null);
      setDialogError(null);
      setExporting(false);
      setFlattening(false);
      setConfirmFlattenUnencrypted(false);
      setProgress(null);
      setPrintPages(null);
      setPrintSize(null);
      setPrintRange("all");
      setCustomFrom("1");
      setCustomTo(String(document.pageCount));
      // Closing the Document unmounts this component; stop any preparation
      // loop that is still awaiting a page render and release its bitmaps.
      return () => {
        cancelPrintRef.current = true;
        preparationGenerationRef.current += 1;
        releaseAllPrintUrls();
      };
    }, [document.documentId, document.pageCount]);

    useEffect(() => {
      if (!dialog) {
        return;
      }
      const onKeyDown = (event: KeyboardEvent) => {
        if (event.key === "Escape" && !preparing) {
          setDialog(null);
        }
      };
      window.addEventListener("keydown", onKeyDown);
      return () => window.removeEventListener("keydown", onKeyDown);
    }, [dialog, preparing]);

    // The native print dialog signals completion with `afterprint`; drop the
    // bitmaps then instead of keeping them alive for the session.
    useEffect(() => {
      if (!printPages) {
        return;
      }
      const clear = () => {
        releaseAllPrintUrls();
        setPrintPages(null);
        setPrintSize(null);
      };
      window.addEventListener("afterprint", clear);
      return () => window.removeEventListener("afterprint", clear);
    }, [printPages]);

    function openPrintDialog() {
      if (policy.printDisabledReason) {
        return;
      }
      setNotice(null);
      setDialogError(null);
      setDialog("print");
    }

    function openExportDialog() {
      setNotice(null);
      setDialogError(null);
      setDialog("export");
    }

    function openFlattenDialog() {
      setNotice(null);
      setDialogError(null);
      setConfirmFlattenUnencrypted(false);
      setDialog("flatten");
    }

    function closeDialog() {
      setDialogError(null);
      setDialog(null);
    }

    function selectedPageIndices(): number[] | null {
      if (printRange === "all") {
        return Array.from({ length: document.pageCount }, (_, index) => index);
      }
      if (printRange === "current") {
        return [pageIndex];
      }
      const from = Number(customFrom);
      const to = Number(customTo);
      if (
        !Number.isInteger(from) ||
        !Number.isInteger(to) ||
        from < 1 ||
        to > document.pageCount ||
        from > to
      ) {
        return null;
      }
      return Array.from({ length: to - from + 1 }, (_, index) => from - 1 + index);
    }

    function cancelPreparation() {
      cancelPrintRef.current = true;
      preparationGenerationRef.current += 1;
      releaseAllPrintUrls();
      setProgress(null);
      setDialogError(null);
      setDialog(null);
      setPrintPages(null);
      setPrintSize(null);
    }

    async function startPrint() {
      const indices = selectedPageIndices();
      if (!indices || indices.length === 0) {
        setDialogError("请输入有效的页面范围。");
        return;
      }

      // Invalidate any earlier preparation loop and release its bitmaps before
      // starting over.
      preparationGenerationRef.current += 1;
      const generation = preparationGenerationRef.current;
      cancelPrintRef.current = false;
      releaseAllPrintUrls();
      setDialogError(null);
      setPrintPages(null);
      setPrintSize(null);
      setProgress({ done: 0, total: indices.length });
      const isCancelled = () =>
        cancelPrintRef.current ||
        preparationGenerationRef.current !== generation;

      const pages: PrintPage[] = [];
      try {
        // Preparation is deliberately sequential: at most one page's base64
        // payload and one decoded RGBA bitmap exist at a time, so a 2000-page
        // job never decodes every page at once. The backend serializes renders
        // on the document registry anyway, so batching would only multiply
        // peak decoded memory without making the job faster.
        for (const [offset, index] of indices.entries()) {
          if (isCancelled()) {
            return;
          }
          const page = await preparePrintPage(
            document.documentId,
            index,
            isCancelled,
          );
          if (!page) {
            return;
          }
          livePrintUrlsRef.current.add(page.imageUrl);
          pages.push(page);
          setProgress({ done: offset + 1, total: indices.length });
        }
      } catch (error) {
        // A stale loop must not release bitmaps owned by a newer print job.
        if (isCancelled()) {
          return;
        }
        releaseAllPrintUrls();
        setProgress(null);
        setDialogError(
          describeError(toCommandError(error), PRINT_ERROR_MESSAGES, "打印准备失败"),
        );
        return;
      }

      if (isCancelled() || pages.length === 0) {
        return;
      }

      // A print job has one sheet size, chosen from the first selected Page;
      // every Page is fitted or printed at actual size inside that sheet.
      // Commit the print surface before asking the platform to print it.
      flushSync(() => {
        setPrintPages(pages);
        setPrintSize(printPageBoxSize(pages[0], orientation));
        setProgress(null);
      });

      try {
        await printDocument(document.documentId);
        if (isCancelled()) {
          return;
        }
        setNotice(`已准备打印：${pages.length} 页`);
        setDialog(null);
      } catch (error) {
        // A stale job must not clear the surface of a newer print job.
        if (isCancelled()) {
          return;
        }
        releaseAllPrintUrls();
        setPrintPages(null);
        setPrintSize(null);
        setDialogError(
          describeError(toCommandError(error), PRINT_ERROR_MESSAGES, "打印失败"),
        );
      }
    }

    async function startExport() {
      setExporting(true);
      setDialogError(null);
      try {
        const destination = await choosePageImageDestination(
          suggestedFileName(document.name, pageIndex + 1, format),
          format,
        );
        if (!destination) {
          return;
        }
        const result = await exportPage(
          document.documentId,
          pageIndex,
          format,
          dpi,
          destination,
        );
        setNotice(`已导出第 ${pageIndex + 1} 页：${result.path}`);
        setDialog(null);
      } catch (error) {
        setDialogError(
          describeError(toCommandError(error), EXPORT_ERROR_MESSAGES, "导出失败"),
        );
      } finally {
        setExporting(false);
      }
    }

    async function startFlattenExport() {
      setFlattening(true);
      setDialogError(null);
      try {
        const destination = await chooseSaveFile(
          suggestedFlattenedFileName(document.name),
        );
        if (!destination) {
          return;
        }
        const result = await exportFlattenedCopy(
          document.documentId,
          destination,
          confirmFlattenUnencrypted,
        );
        setNotice(flattenNotice(result));
        setDialog(null);
        setConfirmFlattenUnencrypted(false);
      } catch (error) {
        setDialogError(
          describeError(
            toCommandError(error),
            FLATTEN_ERROR_MESSAGES,
            "导出 Flatten 副本失败",
          ),
        );
      } finally {
        setFlattening(false);
      }
    }

    useImperativeHandle(ref, () => ({
      openPrintDialog,
      openExportDialog,
    }));

    return (
      <div className="document-actions">
        <div className="print-export-actions">
          <button
            type="button"
            disabled={policy.printDisabledReason !== null}
            title={policy.printDisabledReason ?? undefined}
            onClick={openPrintDialog}
          >
            打印…
          </button>
          <button type="button" onClick={openExportDialog}>
            导出当前页…
          </button>
          <button type="button" onClick={openFlattenDialog}>
            导出 Flatten 副本…
          </button>
          {policy.printDisabledReason ? (
            <span className="print-disabled-reason">
              {policy.printDisabledReason}
            </span>
          ) : null}
        </div>
        {notice ? (
          <span className="print-export-notice" role="status">
            {notice}
          </span>
        ) : null}

        {dialog === "print" ? (
          <div
            className="print-export-overlay"
            onMouseDown={(event) => {
              if (event.target === event.currentTarget && !preparing) {
                closeDialog();
              }
            }}
          >
            <section
              className="print-export-dialog"
              role="dialog"
              aria-modal="true"
              aria-labelledby="print-export-print-title"
            >
              <h2 id="print-export-print-title">打印</h2>
              <p className="print-export-hint">
                {document.name} · 共 {document.pageCount} 页
              </p>

              <fieldset className="print-export-form-range">
                <legend>页面范围</legend>
                <div className="print-export-range-options">
                  <label>
                    <input
                      type="radio"
                      name="print-range"
                      checked={printRange === "all"}
                      disabled={preparing}
                      onChange={() => setPrintRange("all")}
                    />
                    全部页面（{document.pageCount} 页）
                  </label>
                  <label>
                    <input
                      type="radio"
                      name="print-range"
                      checked={printRange === "current"}
                      disabled={preparing}
                      onChange={() => setPrintRange("current")}
                    />
                    当前页（第 {pageIndex + 1} 页）
                  </label>
                  <label>
                    <input
                      type="radio"
                      name="print-range"
                      checked={printRange === "custom"}
                      disabled={preparing}
                      onChange={() => setPrintRange("custom")}
                    />
                    自定义
                  </label>
                </div>
                {printRange === "custom" ? (
                  <div className="print-export-custom-range">
                    <label htmlFor="print-export-from">起始页</label>
                    <input
                      id="print-export-from"
                      type="number"
                      min={1}
                      max={document.pageCount}
                      value={customFrom}
                      disabled={preparing}
                      onChange={(event) => setCustomFrom(event.target.value)}
                    />
                    <label htmlFor="print-export-to">结束页</label>
                    <input
                      id="print-export-to"
                      type="number"
                      min={1}
                      max={document.pageCount}
                      value={customTo}
                      disabled={preparing}
                      onChange={(event) => setCustomTo(event.target.value)}
                    />
                  </div>
                ) : null}
              </fieldset>

              <div className="print-export-field">
                <label htmlFor="print-export-orientation">方向</label>
                <select
                  id="print-export-orientation"
                  value={orientation}
                  disabled={preparing}
                  onChange={(event) =>
                    setOrientation(event.target.value as PrintOrientation)
                  }
                >
                  <option value="auto">跟随页面</option>
                  <option value="portrait">纵向</option>
                  <option value="landscape">横向</option>
                </select>
              </div>

              <div className="print-export-field">
                <label htmlFor="print-export-scale">缩放</label>
                <select
                  id="print-export-scale"
                  value={printScale}
                  disabled={preparing}
                  onChange={(event) =>
                    setPrintScale(event.target.value as PrintScale)
                  }
                >
                  <option value="fit">适合页面</option>
                  <option value="actual">实际大小</option>
                </select>
              </div>

              {dialogError ? (
                <p className="print-export-error" role="alert">
                  {dialogError}
                </p>
              ) : null}

              {progress ? (
                <div className="print-export-progress">
                  <span>
                    正在准备第{" "}
                    {Math.min(progress.done + 1, progress.total)} /{" "}
                    {progress.total} 页…
                  </span>
                  <progress
                    value={progress.done}
                    max={progress.total}
                    aria-label="打印准备进度"
                  />
                  <div>
                    <button type="button" onClick={cancelPreparation}>
                      取消打印
                    </button>
                  </div>
                </div>
              ) : null}

              <div className="print-export-dialog-actions">
                <button
                  type="button"
                  disabled={preparing}
                  onClick={closeDialog}
                >
                  取消
                </button>
                <button
                  type="button"
                  disabled={preparing}
                  onClick={() => void startPrint()}
                >
                  打印…
                </button>
              </div>
            </section>
          </div>
        ) : null}

        {dialog === "export" ? (
          <div
            className="print-export-overlay"
            onMouseDown={(event) => {
              if (event.target === event.currentTarget && !exporting) {
                closeDialog();
              }
            }}
          >
            <section
              className="print-export-dialog"
              role="dialog"
              aria-modal="true"
              aria-labelledby="print-export-export-title"
            >
              <h2 id="print-export-export-title">导出当前页</h2>
              <p className="print-export-hint">
                第 {pageIndex + 1} 页，共 {document.pageCount} 页 ·{" "}
                {document.name}
              </p>

              <div className="print-export-field">
                <label htmlFor="print-export-format">格式</label>
                <select
                  id="print-export-format"
                  value={format}
                  disabled={exporting}
                  onChange={(event) =>
                    setFormat(event.target.value as PageImageFormat)
                  }
                >
                  <option value="png">PNG</option>
                  <option value="jpg">JPG</option>
                </select>
              </div>

              <div className="print-export-field">
                <label htmlFor="print-export-resolution">分辨率</label>
                <select
                  id="print-export-resolution"
                  value={dpi}
                  disabled={exporting}
                  onChange={(event) => setDpi(Number(event.target.value))}
                >
                  {EXPORT_RESOLUTIONS.map((resolution) => (
                    <option key={resolution} value={resolution}>
                      {resolution} DPI
                      {resolution === 72 ? "（实际大小）" : ""}
                    </option>
                  ))}
                </select>
              </div>

              {dialogError ? (
                <p className="print-export-error" role="alert">
                  {dialogError}
                </p>
              ) : null}

              <div className="print-export-dialog-actions">
                <button
                  type="button"
                  disabled={exporting}
                  onClick={closeDialog}
                >
                  取消
                </button>
                <button
                  type="button"
                  disabled={exporting}
                  onClick={() => void startExport()}
                >
                  {exporting ? "正在导出…" : "导出…"}
                </button>
              </div>
            </section>
          </div>
        ) : null}

        {dialog === "flatten" ? (
          <div
            className="print-export-overlay"
            onMouseDown={(event) => {
              if (event.target === event.currentTarget && !flattening) {
                closeDialog();
              }
            }}
          >
            <section
              className="print-export-dialog"
              role="dialog"
              aria-modal="true"
              aria-labelledby="print-export-flatten-title"
            >
              <h2 id="print-export-flatten-title">导出 Flatten 副本</h2>
              <p className="print-export-hint">
                {document.name} · 共 {document.pageCount} 页
              </p>
              <p className="print-export-hint">
                视觉注释将绘制为页面内容，在任何阅读器中显示一致；链接和表单字段保持可交互。
                原文档不会被修改，导出总是生成新文件。没有页面外观的注释（如便笺弹窗）将被省略。
              </p>

              {document.encrypted ? (
                <div className="print-export-warning">
                  <p>此文档已加密。Flatten 副本将以未加密方式保存。</p>
                  <label>
                    <input
                      type="checkbox"
                      checked={confirmFlattenUnencrypted}
                      disabled={flattening}
                      onChange={(event) =>
                        setConfirmFlattenUnencrypted(event.target.checked)
                      }
                    />
                    我了解副本将不加密，继续导出
                  </label>
                </div>
              ) : null}

              {dialogError ? (
                <p className="print-export-error" role="alert">
                  {dialogError}
                </p>
              ) : null}

              <div className="print-export-dialog-actions">
                <button
                  type="button"
                  disabled={flattening}
                  onClick={closeDialog}
                >
                  取消
                </button>
                <button
                  type="button"
                  disabled={
                    flattening ||
                    (document.encrypted && !confirmFlattenUnencrypted)
                  }
                  onClick={() => void startFlattenExport()}
                >
                  {flattening ? "正在导出…" : "导出…"}
                </button>
              </div>
            </section>
          </div>
        ) : null}

        {printPages && printSize
          ? createPortal(
              <div className="print-root" aria-hidden="true">
                <style>
                  {`@page { size: ${printSize.width}pt ${printSize.height}pt; margin: 0; }`}
                </style>
                {printPages.map((page) => (
                  <div
                    key={page.pageIndex}
                    className="print-page"
                    style={{
                      width: `${printSize.width}pt`,
                      height: `${printSize.height}pt`,
                    }}
                  >
                    <img
                      src={page.imageUrl}
                      alt=""
                      className={
                        printScale === "actual"
                          ? "print-page-image print-page-image--actual"
                          : "print-page-image"
                      }
                      style={
                        printScale === "actual"
                          ? {
                              width: `${page.width}pt`,
                              height: `${page.height}pt`,
                            }
                          : undefined
                      }
                    />
                  </div>
                ))}
              </div>,
              window.document.body,
            )
          : null}
      </div>
    );
  },
);
