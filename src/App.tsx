import { useCallback, useEffect, useRef, useState } from "react";

import { DocumentAccess } from "./components/DocumentAccess";
import { DocumentToolbar, PageSurface, ThumbnailRail } from "./components/PageViewer";
import {
  PrintExport,
  type PrintExportHandle,
} from "./components/PrintExport";
import { SettingsPanel } from "./components/SettingsPanel";
import { PasswordPrompt } from "./components/PasswordPrompt";
import { SaveConfirmDialog } from "./components/SaveConfirmDialog";
import {
  choosePdfFile,
  closeDocument,
  getAppInfo,
  initialDocumentPath,
  onFileDrop,
  onMenuEvent,
  onWindowCloseRequested,
  openDocument,
  openDocumentInNewWindow,
  setWindowTitle,
  toCommandError,
  ZOOM_STEP,
  type AppInfo,
  type CommandError,
  type CommandErrorCode,
  type DocumentView,
} from "./lib/backend";
import { scaleToPercent } from "./lib/pageGeometry";
import { useDocumentSave } from "./lib/useDocumentSave";
import { useDocumentViewer } from "./lib/useDocumentViewer";
import { useSettings } from "./lib/useSettings";

type AppInfoState =
  | { status: "loading" }
  | { status: "ready"; info: AppInfo }
  | { status: "unavailable" };

type EditorState =
  | { status: "empty" }
  | { status: "loading" }
  | { status: "open"; document: DocumentView }
  | { status: "error"; error: CommandError; retryPath: string | null };

interface PasswordPromptState {
  path: string;
  fileName: string;
  error: string | null;
  busy: boolean;
}

const WRONG_PASSWORD_MESSAGE = "密码不正确，请重试。";

/** Basenames the same way the backend names a Document. */
function fileName(path: string): string {
  const segments = path.split(/[\\/]/);
  return segments[segments.length - 1] || path;
}

const ERROR_MESSAGES: Partial<Record<CommandErrorCode, string>> = {
  file_not_found: "找不到文件，它可能已被移动或删除。",
  file_read_failed: "无法读取该文件。",
  permission_denied: "没有读取该文件的权限。",
  pdfium_unavailable: "PDF 引擎未就绪，请先运行 npm run setup:native 并重启应用。",
  password_required: "此 PDF 需要密码。",
  invalid_password: "密码不正确。",
  corrupt_document: "文件已损坏或格式不受支持。",
  render_failed: "无法渲染页面。",
  window_failed: "无法打开新窗口。",
};

function errorMessage(code: CommandErrorCode): string {
  return ERROR_MESSAGES[code] ?? "无法打开该文档。";
}

function statusText(state: AppInfoState): string {
  switch (state.status) {
    case "loading":
      return "正在载入…";
    case "ready":
      return `${state.info.name} ${state.info.version}`;
    case "unavailable":
      return "无法获取应用信息";
  }
}

export function App() {
  const [appInfo, setAppInfo] = useState<AppInfoState>({ status: "loading" });
  const [editor, setEditor] = useState<EditorState>({ status: "empty" });
  const [notice, setNotice] = useState<CommandError | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const settings = useSettings();
  const [passwordPrompt, setPasswordPrompt] = useState<PasswordPromptState | null>(null);

  const editorRef = useRef(editor);
  editorRef.current = editor;
  // Ticket 44 wires the save session's dirty flag into the title marker and
  // the close confirmation; ticket 10 only exposes it from the backend.
  const [dirty] = useState(false);
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;
  const openingRef = useRef(false);
  const printExportRef = useRef<PrintExportHandle>(null);

  const handleViewerError = useCallback((error: CommandError) => {
    const current = editorRef.current;
    if (current.status !== "open") {
      return;
    }
    void closeDocument(current.document.documentId).catch(() => {});
    setNotice(null);
    setEditor({
      status: "error",
      error,
      retryPath: current.document.path,
    });
  }, []);

  const openDocumentView = editor.status === "open" ? editor.document : null;
  const viewer = useDocumentViewer(openDocumentView, handleViewerError);
  const viewerRef = useRef(viewer);
  viewerRef.current = viewer;

  // Save state lives next to the viewer so the save buttons and menu actions
  // share the session's target and last result.
  const save = useDocumentSave(openDocumentView);
  const saveRef = useRef(save);
  saveRef.current = save;

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const info = await getAppInfo();
        if (!cancelled) {
          setAppInfo({ status: "ready", info });
        }
      } catch {
        if (!cancelled) {
          setAppInfo({ status: "unavailable" });
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  async function openPath(path: string, password: string | null = null) {
    if (openingRef.current) {
      return;
    }
    openingRef.current = true;
    let openedDocumentId: string | null = null;
    // A password prompt leaves the workspace in the state it had before the
    // open attempt, so cancelling never leaves a loading or empty document
    // behind.
    const previous = editorRef.current;

    try {
      // One window hosts one Document. Opening a second file from a window that
      // already shows a document gets its own window instead of replacing it.
      if (editorRef.current.status === "open") {
        try {
          await openDocumentInNewWindow(path);
          setNotice(null);
        } catch (error) {
          // Keep the current document visible; only window creation failed.
          setNotice(toCommandError(error));
        }
        return;
      }

      setEditor({ status: "loading" });
      setNotice(null);
      const document = await openDocument(path, password);
      openedDocumentId = document.documentId;
      setPasswordPrompt(null);

      await setWindowTitle(`${document.name} — PDFX`).catch(() => {
        // The title is cosmetic; a missing window capability must not fail open.
      });

      setEditor({ status: "open", document });
    } catch (error) {
      if (openedDocumentId) {
        await closeDocument(openedDocumentId).catch(() => {});
      }

      const commandError = toCommandError(error);
      if (
        commandError.code === "password_required" ||
        commandError.code === "invalid_password"
      ) {
        // Nothing is rendered before a successful open; the prompt is the only
        // surface that survives a failed attempt.
        setEditor(previous);
        setPasswordPrompt({
          path,
          fileName: fileName(path),
          error: commandError.code === "invalid_password" ? WRONG_PASSWORD_MESSAGE : null,
          busy: false,
        });
        return;
      }

      setPasswordPrompt(null);
      setNotice(null);
      setEditor({
        status: "error",
        error: commandError,
        retryPath: path,
      });
    } finally {
      openingRef.current = false;
    }
  }

  function submitPassword(password: string) {
    if (!passwordPrompt || passwordPrompt.busy) {
      return;
    }
    const prompt = passwordPrompt;
    setPasswordPrompt({ ...prompt, busy: true, error: null });
    void openPath(prompt.path, password);
  }

  function cancelPasswordPrompt() {
    setPasswordPrompt(null);
  }

  function promptOpen() {
    void (async () => {
      try {
        const path = await choosePdfFile();
        if (path) {
          await openPath(path);
        }
      } catch (error) {
        console.warn("[pdfx] could not show the file dialog", error);
      }
    })();
  }

  function handleMenu(menuId: string) {
    const current = viewerRef.current;
    switch (menuId) {
      case "file.open":
        promptOpen();
        break;
      case "file.print":
        printExportRef.current?.openPrintDialog();
        break;
      case "file.export_page":
        printExportRef.current?.openExportDialog();
        break;
      case "file.save":
        saveRef.current.saveChanges();
        break;
      case "file.save_as":
        saveRef.current.saveAs();
        break;
      case "view.zoom_in":
        current.zoomBy(ZOOM_STEP);
        break;
      case "view.zoom_out":
        current.zoomBy(1 / ZOOM_STEP);
        break;
      case "view.actual_size":
        current.actualSize();
        break;
      case "view.fit_width":
        current.fit("width");
        break;
      case "view.fit_page":
        current.fit("page");
        break;
      case "view.fullscreen":
        current.toggleFullscreen();
        break;
      case "app.settings":
        setSettingsOpen(true);
        break;
      default:
        break;
    }
  }

  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];

    const subscribe = (subscription: Promise<() => void>) => {
      subscription
        .then((unlisten) => {
          if (disposed) {
            unlisten();
          } else {
            unlisteners.push(unlisten);
          }
        })
        .catch(() => {
          // Outside the Tauri shell these listeners are unavailable.
        });
    };

    try {
      subscribe(onMenuEvent(handleMenu));
      subscribe(
        onFileDrop((paths) => {
          const pdf = paths.find((path) => path.toLowerCase().endsWith(".pdf"));
          if (pdf) {
            void openPath(pdf);
          }
        }),
      );
      subscribe(
        onWindowCloseRequested((event) => {
          if (dirtyRef.current) {
            // Confirm or save before closing once editing exists.
            event.preventDefault();
            return;
          }
          const current = editorRef.current;
          if (current.status === "open") {
            void closeDocument(current.document.documentId).catch(() => {});
          }
        }),
      );
    } catch {
      // Running in a plain browser (Vite dev without Tauri) is supported.
    }

    return () => {
      disposed = true;
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
    // Handlers read live state through refs, so one subscription is enough.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const path = initialDocumentPath();
    if (path) {
      void openPath(path);
    }
    // A new window opens exactly the document named in its launch URL.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const open = editor.status === "open" ? editor : null;

  return (
    <div className="app-shell">
      <header className="app-header">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true">
            P
          </span>
          <h1>PDFX</h1>
        </div>
        <div className="header-actions">
          {open && (
            <span className="document-name" title={open.document.path}>
              {open.document.name}
            </span>
          )}
          <span className="app-mode">本地 PDF 编辑器</span>
          <button
            type="button"
            className="secondary-button"
            onClick={() => setSettingsOpen(true)}
          >
            设置
          </button>
          <button type="button" className="open-button" onClick={promptOpen}>
            打开 PDF…
          </button>
        </div>
      </header>

      {notice && (
        <div className="app-notice" role="alert">
          <span>{errorMessage(notice.code)}</span>
          <button type="button" onClick={() => setNotice(null)}>
            关闭
          </button>
        </div>
      )}

      {passwordPrompt && (
        <PasswordPrompt
          fileName={passwordPrompt.fileName}
          error={passwordPrompt.error}
          busy={passwordPrompt.busy}
          onSubmit={submitPassword}
          onCancel={cancelPasswordPrompt}
        />
      )}

      {open && save.confirmation && (
        <SaveConfirmDialog
          documentName={open.document.name}
          busy={save.busy}
          onConfirm={save.confirmUnencrypted}
          onCancel={save.cancelConfirmation}
        />
      )}

      <main className="workspace" aria-label="PDFX 工作区">
        {editor.status === "empty" && (
          <section className="empty-workspace">
            <h2>空工作区</h2>
            <p>打开一个 PDF 文件开始编辑。文件只在本机处理。</p>
            <button type="button" className="secondary-button" onClick={promptOpen}>
              选择 PDF 文件
            </button>
          </section>
        )}

        {editor.status === "loading" && (
          <div className="loading-state">正在打开…</div>
        )}

        {editor.status === "error" && (
          <section className="error-panel" role="alert">
            <h2>无法打开文档</h2>
            <p>{errorMessage(editor.error.code)}</p>
            <p className="error-detail">{editor.error.message}</p>
            {editor.retryPath && (
              <button
                type="button"
                className="secondary-button"
                onClick={() => {
                  const path = editor.retryPath;
                  if (path) {
                    void openPath(path);
                  }
                }}
              >
                重试
              </button>
            )}
          </section>
        )}

        {open && (
          <section className="document-view" aria-label="文档视图">
            <DocumentToolbar
              pageIndex={viewer.pageIndex}
              pageCount={open.document.pageCount}
              scale={viewer.scale}
              zoomMode={viewer.zoomMode}
              fullscreen={viewer.fullscreen}
              onSelectPage={viewer.goToPage}
              onZoomTo={viewer.zoomTo}
              onZoomIn={() => viewer.zoomBy(ZOOM_STEP)}
              onZoomOut={() => viewer.zoomBy(1 / ZOOM_STEP)}
              onFit={viewer.fit}
              onActualSize={viewer.actualSize}
              onToggleFullscreen={viewer.toggleFullscreen}
            />
            <PrintExport
              ref={printExportRef}
              document={open.document}
              pageIndex={viewer.pageIndex}
            />
            <DocumentAccess document={open.document} save={save} />
            <div className="document-body">
              <ThumbnailRail
                documentId={open.document.documentId}
                pageCount={open.document.pageCount}
                currentPageIndex={viewer.pageIndex}
                onSelectPage={viewer.goToPage}
              />
              <div
                ref={viewer.viewportRef}
                className="page-stage"
                role="region"
                aria-label="页面视口"
                tabIndex={0}
                onScroll={viewer.handleScroll}
                onKeyDown={viewer.handleViewportKeyDown}
              >
                <div className="page-stage-inner">
                  <PageSurface
                    documentId={open.document.documentId}
                    pageIndex={viewer.pageIndex}
                    info={viewer.info}
                    scale={viewer.scale}
                    render={viewer.render}
                    readOnly={open.document.readOnly}
                    surfaceRef={viewer.surfaceRef}
                  />
                </div>
              </div>
            </div>
          </section>
        )}
      </main>

      <footer className="status-bar">
        <span role="status">{statusText(appInfo)}</span>
        {open && (
          <span className="document-status">
            第 {viewer.pageIndex + 1} / {open.document.pageCount} 页 ·{" "}
            {scaleToPercent(viewer.scale)}%
          </span>
        )}
        <span>离线 · 无遥测</span>
      </footer>

      <SettingsPanel
        open={settingsOpen}
        settings={settings.settings}
        onClose={() => setSettingsOpen(false)}
        onSave={settings.update}
      />
    </div>
  );
}
