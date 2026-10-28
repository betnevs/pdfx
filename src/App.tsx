import { useEffect, useRef, useState } from "react";

import { DocumentAccess } from "./components/DocumentAccess";
import { PasswordPrompt } from "./components/PasswordPrompt";
import {
  choosePdfFile,
  closeDocument,
  DEFAULT_SCALE,
  getAppInfo,
  initialDocumentPath,
  MAX_SCALE,
  MIN_SCALE,
  onFileDrop,
  onMenuEvent,
  onWindowCloseRequested,
  openDocument,
  openDocumentInNewWindow,
  renderPage,
  setWindowTitle,
  toCommandError,
  ZOOM_STEP,
  type AppInfo,
  type CommandError,
  type CommandErrorCode,
  type DocumentView,
  type PageImageView,
} from "./lib/backend";

type AppInfoState =
  | { status: "loading" }
  | { status: "ready"; info: AppInfo }
  | { status: "unavailable" };

type EditorState =
  | { status: "empty" }
  | { status: "loading" }
  | { status: "open"; document: DocumentView; page: PageImageView; zoom: number }
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

function clampScale(scale: number): number {
  const bounded = Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
  return Math.round(bounded * 1000) / 1000;
}

export function App() {
  const [appInfo, setAppInfo] = useState<AppInfoState>({ status: "loading" });
  const [editor, setEditor] = useState<EditorState>({ status: "empty" });
  const [notice, setNotice] = useState<CommandError | null>(null);
  const [passwordPrompt, setPasswordPrompt] = useState<PasswordPromptState | null>(null);

  // Editing arrives in a later ticket; the scaffold tracks dirty state and the
  // close guard so nothing silently discards future unsaved changes.
  const [dirty] = useState(false);

  const editorRef = useRef(editor);
  editorRef.current = editor;
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;
  const openingRef = useRef(false);

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

      const page = await renderPage(document.documentId, 0, DEFAULT_SCALE);
      setEditor({ status: "open", document, page, zoom: DEFAULT_SCALE });
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

  async function zoomTo(requested: number) {
    const current = editorRef.current;
    if (current.status !== "open") {
      return;
    }

    const scale = clampScale(requested);
    if (scale === current.zoom) {
      return;
    }

    try {
      const page = await renderPage(current.document.documentId, 0, scale);
      const latest = editorRef.current;
      if (
        latest.status === "open" &&
        latest.document.documentId === current.document.documentId
      ) {
        setEditor({ ...latest, page, zoom: scale });
      }
    } catch (error) {
      await closeDocument(current.document.documentId).catch(() => {});
      setNotice(null);
      setEditor({
        status: "error",
        error: toCommandError(error),
        retryPath: current.document.path,
      });
    }
  }

  function zoomBy(factor: number) {
    const current = editorRef.current;
    if (current.status === "open") {
      void zoomTo(current.zoom * factor);
    }
  }

  function handleMenu(menuId: string) {
    switch (menuId) {
      case "file.open":
        promptOpen();
        break;
      case "view.zoom_in":
        zoomBy(ZOOM_STEP);
        break;
      case "view.zoom_out":
        zoomBy(1 / ZOOM_STEP);
        break;
      case "view.actual_size":
        void zoomTo(DEFAULT_SCALE);
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
            <div className="document-toolbar">
              <button type="button" onClick={() => zoomBy(1 / ZOOM_STEP)}>
                缩小
              </button>
              <span className="zoom-value">{Math.round(open.zoom * 100)}%</span>
              <button type="button" onClick={() => zoomBy(ZOOM_STEP)}>
                放大
              </button>
              <button type="button" onClick={() => void zoomTo(DEFAULT_SCALE)}>
                实际大小
              </button>
            </div>
            <DocumentAccess document={open.document} />
            <div className="page-stage">
              <PageCanvas page={open.page} />
            </div>
          </section>
        )}
      </main>

      <footer className="status-bar">
        <span role="status">{statusText(appInfo)}</span>
        {open && (
          <span className="document-status">
            第 {open.page.pageIndex + 1} / {open.document.pageCount} 页 ·{" "}
            {Math.round(open.zoom * 100)}%
          </span>
        )}
        <span>离线 · 无遥测</span>
      </footer>
    </div>
  );
}

function PageCanvas({ page }: { page: PageImageView }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) {
      return;
    }

    const image = new Image();
    image.onload = () => {
      const context = canvas.getContext("2d");
      if (!context) {
        return;
      }
      canvas.width = page.width;
      canvas.height = page.height;
      context.clearRect(0, 0, page.width, page.height);
      context.drawImage(image, 0, 0, page.width, page.height);
    };
    image.src = `data:image/png;base64,${page.pngBase64}`;

    return () => {
      image.onload = null;
    };
  }, [page]);

  return (
    <canvas
      ref={canvasRef}
      className="page-canvas"
      role="img"
      aria-label={`第 ${page.pageIndex + 1} 页`}
      width={page.width}
      height={page.height}
    />
  );
}
