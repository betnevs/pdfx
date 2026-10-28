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
  exportPage,
  pageInfo,
  printDocument,
  renderPage,
  toCommandError,
  type CommandError,
  type DocumentView,
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

type Dialog = "print" | "export";
type PrintRange = "all" | "current" | "custom";
type PrintOrientation = "auto" | "portrait" | "landscape";
type PrintScale = "fit" | "actual";

interface PrintPage {
  pageIndex: number;
  pngBase64: string;
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
  invalid_argument: "不能覆盖源 PDF 文件，请选择其他位置。",
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
 * Decodes every print bitmap before the native print operation starts, so the
 * first page never prints blank. Browsers without `HTMLImageElement.decode`
 * (the jsdom test environment) skip this step.
 */
async function decodePrintImages(pages: PrintPage[]): Promise<void> {
  if (typeof Image === "undefined") {
    return;
  }
  const probe = new Image();
  if (typeof probe.decode !== "function") {
    return;
  }
  await Promise.all(
    pages.map(
      (page) =>
        new Promise<void>((resolve) => {
          const image = new Image();
          image.onload = () => resolve();
          image.onerror = () => resolve();
          image.src = `data:image/png;base64,${page.pngBase64}`;
          image.decode().then(
            () => resolve(),
            () => resolve(),
          );
        }),
    ),
  );
}

/**
 * Print and page-export actions for the open Document.
 *
 * Both are read-only: they render through the same commands as the page
 * viewer and never change the Document or its unsaved state. Export is always
 * available; printing is disabled when the Document's Permission Restrictions
 * deny it.
 *
 * Printing renders the selected Pages into a hidden print-only surface (one
 * bitmap per Page, a `@page` box in points and a page break between Pages).
 * The platform dialog therefore offers the correct page range, orientation
 * and scaling options. Preparation reports progress and can be cancelled,
 * which keeps large Documents usable.
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
    const preparing = progress !== null;

    // A new Document invalidates everything prepared for the previous one.
    useEffect(() => {
      cancelPrintRef.current = true;
      setDialog(null);
      setNotice(null);
      setDialogError(null);
      setExporting(false);
      setProgress(null);
      setPrintPages(null);
      setPrintSize(null);
      setPrintRange("all");
      setCustomFrom("1");
      setCustomTo(String(document.pageCount));
      // Closing the Document unmounts this component; stop any preparation
      // loop that is still awaiting a page render.
      return () => {
        cancelPrintRef.current = true;
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
      setProgress(null);
      setDialogError(null);
      setDialog(null);
    }

    async function startPrint() {
      const indices = selectedPageIndices();
      if (!indices || indices.length === 0) {
        setDialogError("请输入有效的页面范围。");
        return;
      }

      cancelPrintRef.current = false;
      setDialogError(null);
      setPrintPages(null);
      setPrintSize(null);
      setProgress({ done: 0, total: indices.length });

      const pages: PrintPage[] = [];
      try {
        for (const [offset, index] of indices.entries()) {
          const [image, info] = await Promise.all([
            renderPage(document.documentId, index, PRINT_DPI / 72),
            pageInfo(document.documentId, index),
          ]);
          if (cancelPrintRef.current) {
            return;
          }
          pages.push({
            pageIndex: index,
            pngBase64: image.pngBase64,
            width: info.width,
            height: info.height,
          });
          setProgress({ done: offset + 1, total: indices.length });
        }
      } catch (error) {
        if (cancelPrintRef.current) {
          return;
        }
        setProgress(null);
        setDialogError(
          describeError(toCommandError(error), PRINT_ERROR_MESSAGES, "打印准备失败"),
        );
        return;
      }

      if (cancelPrintRef.current || pages.length === 0) {
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
        await decodePrintImages(pages);
        await printDocument(document.documentId);
        setNotice(`已准备打印：${pages.length} 页`);
        setDialog(null);
      } catch (error) {
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
                      src={`data:image/png;base64,${page.pngBase64}`}
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
