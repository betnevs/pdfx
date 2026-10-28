import { useCallback, useEffect, useRef, useState } from "react";

import { DocumentAccess } from "./components/DocumentAccess";
import { AnnotationPanel } from "./components/AnnotationPanel";
import { StampPanel } from "./components/StampPanel";
import { DocumentToolbar, PageSurface, ThumbnailRail } from "./components/PageViewer";
import {
  PrintExport,
  type PrintExportHandle,
} from "./components/PrintExport";
import { SettingsPanel } from "./components/SettingsPanel";
import { PasswordPrompt } from "./components/PasswordPrompt";
import { CloseConfirmDialog } from "./components/CloseConfirmDialog";
import {
  DeletePageDialog,
  InsertBlankPageDialog,
} from "./components/PageOperationDialogs";
import { RecoveryPrompt } from "./components/RecoveryPrompt";
import { SaveConfirmDialog } from "./components/SaveConfirmDialog";
import {
  choosePdfFile,
  closeDocument,
  deletePage,
  destroyWindow,
  editHistory,
  getAppInfo,
  initialDocumentPath,
  insertBlankPage,
  movePage,
  onFileDrop,
  onMenuEvent,
  onWindowCloseRequested,
  openDocument,
  openDocumentInNewWindow,
  redoTextEdit,
  rotatePage,
  setMenuState,
  setWindowTitle,
  toCommandError,
  undoTextEdit,
  ZOOM_STEP,
  type AnnotationView,
  type AppInfo,
  type BlankPageSize,
  type CommandError,
  type CommandErrorCode,
  type DocumentView,
  type EditHistoryView,
  type FormFieldView,
  type PageOperationView,
  type StampKind,
} from "./lib/backend";
import { scaleToPercent } from "./lib/pageGeometry";
import { notifyDocumentChanged } from "./lib/documentEvents";
import { isEditableTarget, isWindowsPlatform } from "./lib/keyboard";
import {
  MENU_ITEM_IDS,
  menuItemStates,
  menuStateSignature,
} from "./lib/menu";
import { useAnnotations } from "./lib/useAnnotations";
import { useDocumentSave } from "./lib/useDocumentSave";
import { useDocumentViewer } from "./lib/useDocumentViewer";
import { useRecoverySnapshots, useRecoveryStartup } from "./lib/useRecovery";
import { useRecentFiles } from "./lib/useRecentFiles";
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

/** The unsaved-changes question shown before a dirty window closes. */
interface ClosePromptState {
  /** True while the close-triggered save is running. */
  busy: boolean;
  /** Explains why the last save attempt left the window open. */
  message: string | null;
}

/** A page operation waiting for the user's confirmation or paper size. */
type PageDialogState =
  | { kind: "delete"; pageIndex: number }
  | {
      kind: "insert";
      /** Logical insertion slot: right after the page shown when opening. */
      afterIndex: number;
      /** Display size of the page the dialog was opened from. */
      currentSize: BlankPageSize | null;
    };

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
  document_not_found: "文档已关闭。",
  render_failed: "无法渲染页面。",
  window_failed: "无法打开新窗口。",
  recovery_failed: "无法处理恢复文件，请重试。",
  recovery_not_found: "恢复文件已不存在，可能已被删除或恢复。",
  internal: "操作失败，请重试。",
};

function errorMessage(code: CommandErrorCode): string {
  return ERROR_MESSAGES[code] ?? "无法打开该文档。";
}

/**
 * Menu commands a focused text control owns.
 *
 * Disabling these items also releases their native accelerators, but that
 * snapshot reaches the menu asynchronously. When an accelerator event still
 * arrives in the gap, the focused control must keep the key: Cmd/Ctrl+Z and
 * PageUp/PageDown belong to typing and scrolling there, not to the document.
 */
const TEXT_OWNED_MENU_IDS: ReadonlySet<string> = new Set<string>([
  MENU_ITEM_IDS.undo,
  MENU_ITEM_IDS.redo,
  MENU_ITEM_IDS.previousPage,
  MENU_ITEM_IDS.nextPage,
]);

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
  // A `?open=` window belongs to one Document; the start-up recovery question
  // belongs to the main window only.
  const launchedWithDocument = useRef(initialDocumentPath() !== null).current;
  const recovery = useRecoveryStartup(!launchedWithDocument);
  const recent = useRecentFiles(!launchedWithDocument);
  const [passwordPrompt, setPasswordPrompt] = useState<PasswordPromptState | null>(null);
  const [pageDialog, setPageDialog] = useState<PageDialogState | null>(null);

  const editorRef = useRef(editor);
  editorRef.current = editor;
  const [closePrompt, setClosePrompt] = useState<ClosePromptState | null>(null);
  const closePromptRef = useRef(closePrompt);
  closePromptRef.current = closePrompt;
  // Set once the guard has released the Document and asked the window to go
  // away; further close events must not re-enter the confirmation flow.
  const closingRef = useRef(false);
  const closeSaveInFlightRef = useRef(false);
  const openingRef = useRef(false);
  const pageOperationInFlightRef = useRef(false);
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

  const annotations = useAnnotations(openDocumentView?.documentId ?? null);
  const [selectedAnnotation, setSelectedAnnotation] =
    useState<AnnotationView | null>(null);
  const [armedStamp, setArmedStamp] = useState<StampKind | null>(null);

  // A list refresh can remove the selected annotation (delete, undo). Drop the
  // outline with it; the panel clears its property controls the same way.
  useEffect(() => {
    setSelectedAnnotation((current) =>
      current === null
        ? null
        : (annotations.annotations.find(
            (annotation) => annotation.name === current.name,
          ) ?? null),
    );
  }, [annotations.annotations]);

  useEffect(() => {
    setSelectedAnnotation(null);
    setArmedStamp(null);
  }, [openDocumentView?.documentId]);

  function selectAnnotation(annotation: AnnotationView | null) {
    setSelectedAnnotation(annotation);
    setArmedStamp(null);
    if (annotation) {
      viewer.goToPage(annotation.pageIndex);
    }
  }

  function handleStampPlaced(annotation: AnnotationView) {
    setArmedStamp(null);
    setSelectedAnnotation(annotation);
  }

  // Required-field validation navigates to the page of the first empty widget
  // and asks the form overlay to focus it.
  const [formFocusFieldId, setFormFocusFieldId] = useState<string | null>(null);
  const locateField = useCallback((field: FormFieldView) => {
    viewerRef.current.goToPage(field.pageIndex);
    setFormFocusFieldId(field.fieldId);
  }, []);
  const handleFormFocusHandled = useCallback(() => {
    setFormFocusFieldId(null);
  }, []);

  // Save state lives next to the viewer so the save buttons and menu actions
  // share the session's target and last result.
  const save = useDocumentSave(openDocumentView, {
    onMissingRequired: (missing) => {
      const first = missing[0];
      if (first) {
        locateField(first);
      }
    },
    // Every successful save also becomes a recent file.
    onSaved: () => {
      void recent.refresh();
    },
  });
  const saveRef = useRef(save);
  saveRef.current = save;

  // Page operations follow the same content-modification permission gate as
  // the Rust commands: a read-only document shows disabled controls and a
  // Chinese explanation instead of mutating history.
  const canModifyPages =
    (openDocumentView?.permissions.canModifyContent ?? false) &&
    !(openDocumentView?.readOnly ?? false);
  const pageOperationDisabledReason = !openDocumentView
    ? null
    : openDocumentView.readOnly
      ? "文档为只读，页面操作已禁用。"
      : !openDocumentView.permissions.canModifyContent
        ? "当前文档的权限不允许修改页面内容。"
        : null;

  /**
   * Runs one page operation and publishes its outcome: new page count,
   * unified history, and a structure-change event so page metadata and every
   * thumbnail cache refresh. Selection follows the operation's page.
   */
  const applyPageOperation = useCallback(
    async (operation: () => Promise<PageOperationView>) => {
      if (pageOperationInFlightRef.current) {
        return;
      }
      const current = editorRef.current;
      if (current.status !== "open") {
        return;
      }
      const { document } = current;
      if (document.readOnly || !document.permissions.canModifyContent) {
        return;
      }
      pageOperationInFlightRef.current = true;
      try {
        const result = await operation();
        saveRef.current.acceptHistory(result.history);
        setEditor((state) =>
          state.status === "open" &&
          state.document.documentId === document.documentId
            ? {
                status: "open",
                document: { ...state.document, pageCount: result.pageCount },
              }
            : state,
        );
        notifyDocumentChanged({
          documentId: document.documentId,
          pageIndex: result.pageIndex,
          pageCount: result.pageCount,
          pageStructureChanged: true,
        });
      } catch (error) {
        setNotice(toCommandError(error));
      } finally {
        pageOperationInFlightRef.current = false;
      }
    },
    [],
  );

  const rotateCurrentPage = useCallback(
    (deltaDegrees: 90 | 180 | 270) => {
      const current = editorRef.current;
      if (current.status !== "open") {
        return;
      }
      if (current.document.readOnly || !current.document.permissions.canModifyContent) {
        return;
      }
      const documentId = current.document.documentId;
      const pageIndex = viewerRef.current.pageIndex;
      void applyPageOperation(() =>
        rotatePage(documentId, pageIndex, deltaDegrees),
      );
    },
    [applyPageOperation],
  );

  const moveThumbnailPage = useCallback(
    (fromIndex: number, toIndex: number) => {
      const current = editorRef.current;
      if (current.status !== "open") {
        return;
      }
      if (current.document.readOnly || !current.document.permissions.canModifyContent) {
        return;
      }
      const documentId = current.document.documentId;
      void applyPageOperation(() =>
        movePage(documentId, fromIndex, toIndex),
      );
    },
    [applyPageOperation],
  );

  function confirmDeletePage() {
    const dialog = pageDialog;
    setPageDialog(null);
    if (!dialog || dialog.kind !== "delete") {
      return;
    }
    const current = editorRef.current;
    if (current.status !== "open") {
      return;
    }
    const documentId = current.document.documentId;
    const { pageIndex } = dialog;
    void applyPageOperation(() => deletePage(documentId, pageIndex));
  }

  function confirmInsertBlankPage(size: BlankPageSize) {
    const dialog = pageDialog;
    setPageDialog(null);
    if (!dialog || dialog.kind !== "insert") {
      return;
    }
    const current = editorRef.current;
    if (current.status !== "open") {
      return;
    }
    const documentId = current.document.documentId;
    const { afterIndex } = dialog;
    void applyPageOperation(() =>
      insertBlankPage(documentId, afterIndex, size),
    );
  }

  function openInsertBlankPageDialog() {
    if (!canModifyPages) {
      return;
    }
    setPageDialog({
      kind: "insert",
      afterIndex: viewer.pageIndex + 1,
      currentSize: viewer.info
        ? { width: viewer.info.width, height: viewer.info.height }
        : null,
    });
  }

  function openDeletePageDialog() {
    if (!canModifyPages) {
      return;
    }
    setPageDialog({ kind: "delete", pageIndex: viewer.pageIndex });
  }

  // While the open Document is dirty, ask the backend for throttled Recovery
  // File snapshots. The backend rejects writes while nothing changed.
  useRecoverySnapshots(openDocumentView?.documentId ?? null, save.dirty);

  // The recovery question is modal and belongs to the empty start screen; it
  // keeps the menu unavailable until the user restores, discards or dismisses.
  const recoveryPromptOpen =
    !launchedWithDocument &&
    editor.status === "empty" &&
    recovery.open &&
    recovery.candidates.length > 0;
  // The drop subscription is created once; read the modal state through a ref
  // so a dropped file cannot answer the recovery question by accident.
  const recoveryPromptOpenRef = useRef(recoveryPromptOpen);
  recoveryPromptOpenRef.current = recoveryPromptOpen;

  // A focused text control owns Cmd/Ctrl+Z, PageUp/PageDown and Home/End. The
  // menu state below disables those items, which also releases their native
  // accelerators, so the keys reach the control and the platform's own undo.
  const [textInputFocused, setTextInputFocused] = useState(false);
  useEffect(() => {
    const onFocusIn = (event: FocusEvent) => {
      setTextInputFocused(isEditableTarget(event.target));
    };
    const onFocusOut = () => {
      setTextInputFocused(false);
    };
    document.addEventListener("focusin", onFocusIn);
    document.addEventListener("focusout", onFocusOut);
    return () => {
      document.removeEventListener("focusin", onFocusIn);
      document.removeEventListener("focusout", onFocusOut);
    };
  }, []);

  // The native menu lives in Rust. This snapshot is the single source of truth
  // for which items can act; see `src/lib/menu.ts` for the rules. `menuEnabled`
  // is also read when a menu event arrives, so an event that raced ahead of a
  // state push can never run a command that is currently unavailable.
  const menuState = menuItemStates({
    hasDocument: openDocumentView !== null,
    readOnly: openDocumentView?.readOnly ?? false,
    canPrint: openDocumentView?.permissions.canPrint ?? false,
    canUndo: save.canUndo,
    canRedo: save.canRedo,
    saving: save.busy,
    modalOpen:
      closePrompt !== null ||
      passwordPrompt !== null ||
      save.confirmation !== null ||
      recoveryPromptOpen ||
      settingsOpen ||
      pageDialog !== null,
    textInputFocused,
    pageIndex: viewer.pageIndex,
    pageCount: openDocumentView?.pageCount ?? 0,
  });
  const menuSignature = menuStateSignature(menuState);
  const menuEnabledRef = useRef<Map<string, boolean>>(new Map());
  menuEnabledRef.current = new Map(
    menuState.map((item) => [item.id, item.enabled]),
  );
  // The native menu is app-wide, so two document windows can disagree about
  // what is available. Re-publishing when this window gains focus makes the
  // menu reflect the window the user is actually working in.
  const [menuPushRevision, setMenuPushRevision] = useState(0);
  useEffect(() => {
    const onWindowFocus = () => setMenuPushRevision((value) => value + 1);
    window.addEventListener("focus", onWindowFocus);
    return () => window.removeEventListener("focus", onWindowFocus);
  }, []);

  const lastMenuPushRef = useRef<string | null>(null);
  useEffect(() => {
    const pushKey = `${menuSignature}:${menuPushRevision}`;
    if (lastMenuPushRef.current === pushKey) {
      return;
    }
    lastMenuPushRef.current = pushKey;
    void setMenuState(menuState).catch(() => {
      // Outside the Tauri shell the command does not exist; a later state
      // change retries so a transient failure cannot leave the menu stale.
      lastMenuPushRef.current = null;
    });
    // `menuState` is rebuilt every render; the signature guard decides whether
    // anything actually changed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [menuSignature, menuPushRevision]);

  const historyActionInFlightRef = useRef(false);
  const runHistoryAction = useCallback(async (kind: "undo" | "redo") => {
    if (historyActionInFlightRef.current) {
      return;
    }
    const current = editorRef.current;
    if (current.status !== "open") {
      return;
    }
    const documentId = current.document.documentId;
    const currentSave = saveRef.current;
    if (
      currentSave.busy ||
      (kind === "undo" ? !currentSave.canUndo : !currentSave.canRedo)
    ) {
      return;
    }
    historyActionInFlightRef.current = true;
    try {
      const step =
        kind === "undo"
          ? await undoTextEdit(documentId)
          : await redoTextEdit(documentId);
      currentSave.acceptHistory(step.history);
      setEditor((state) =>
        state.status === "open" &&
        state.document.documentId === documentId
          ? {
              status: "open",
              document: { ...state.document, pageCount: step.pageCount },
            }
          : state,
      );
      // The Edit Session is document-wide: refresh every overlay, the page
      // bitmap and the dirty marker, not just the page that changed. Undoing
      // a page operation also renumbers every slot.
      notifyDocumentChanged({
        documentId,
        pageIndex: step.pageIndex,
        pageCount: step.pageCount,
        pageStructureChanged: step.pageStructureChanged,
      });
    } catch (error) {
      setNotice(toCommandError(error));
    } finally {
      historyActionInFlightRef.current = false;
    }
  }, []);

  // Windows also accepts Ctrl+Y for redo. The menu item carries a single
  // accelerator (CmdOrCtrl+Shift+Z); this fallback only acts when the item is
  // enabled, so it cannot redo twice or steal the key from a text control.
  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        !isWindowsPlatform() ||
        !event.ctrlKey ||
        event.metaKey ||
        event.altKey ||
        event.shiftKey ||
        event.key.toLowerCase() !== "y" ||
        isEditableTarget(event.target) ||
        !menuEnabledRef.current.get("edit.redo")
      ) {
        return;
      }
      event.preventDefault();
      void runHistoryAction("redo");
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [runHistoryAction]);

  // Story 10: the native title carries the Document name and an unsaved
  // marker. The marker comes from the backend's dirty flag only; the save
  // hook re-reads it after every edit, undo and redo.
  const openDocumentId = openDocumentView?.documentId ?? null;
  const openDocumentName = openDocumentView?.name ?? null;
  const lastTitleRef = useRef<string | null>(null);
  useEffect(() => {
    if (!openDocumentName) {
      // A document that was closed after an error no longer backs the title.
      if (lastTitleRef.current !== null) {
        lastTitleRef.current = null;
        void setWindowTitle("PDFX").catch(() => {});
      }
      return;
    }
    const marker = save.dirty ? "（未保存）" : "";
    const title = `${openDocumentName}${marker} — PDFX`;
    if (lastTitleRef.current === title) {
      return;
    }
    lastTitleRef.current = title;
    void setWindowTitle(title).catch(() => {
      // The title is cosmetic; a missing window capability must not fail open.
    });
  }, [openDocumentId, openDocumentName, save.dirty]);

  /**
   * Releases the Document and destroys the window without another close
   * round trip.
   */
  const closeWindow = useCallback(async () => {
    if (closingRef.current) {
      return;
    }
    closingRef.current = true;
    const current = editorRef.current;
    if (current.status === "open") {
      await closeDocument(current.document.documentId).catch(() => {});
    }
    closePromptRef.current = null;
    setClosePrompt(null);
    try {
      await destroyWindow();
    } catch (error) {
      // The window survived; allow a later close attempt to run the guard.
      closingRef.current = false;
      console.warn("[pdfx] could not close the window", error);
    }
  }, []);

  /**
   * The OS close request. The default close is always prevented first so the
   * decision can wait on a fresh backend read; a clean window is then closed
   * explicitly. An edit whose page-change event has not arrived yet is still
   * seen because `edit_history` is queried at the command boundary.
   */
  const handleCloseRequested = useCallback(
    async (event: { preventDefault: () => void }) => {
      event.preventDefault();
      if (closingRef.current || closePromptRef.current) {
        // Already closing, or already asking. Repeated close clicks must not
        // stack dialogs or start a second save.
        return;
      }
      const current = editorRef.current;
      if (current.status !== "open") {
        await closeWindow();
        return;
      }
      const documentId = current.document.documentId;
      // A save started from the menu may still be writing. Let it settle so
      // the read below reflects the backend's command-boundary state.
      await saveRef.current.waitForIdle();
      if (closingRef.current || closePromptRef.current) {
        return;
      }
      let history: EditHistoryView | null = null;
      try {
        history = await editHistory(documentId);
      } catch {
        // Without an answer, never discard changes silently: ask the user.
      }
      if (closingRef.current || closePromptRef.current) {
        return;
      }
      if (history && !history.dirty) {
        await closeWindow();
        return;
      }
      // A confirmation left over from an earlier save attempt belongs to that
      // attempt; the close dialog re-raises it if the user chooses Save.
      saveRef.current.cancelConfirmation();
      setClosePrompt({ busy: false, message: null });
    },
    [closeWindow],
  );

  /**
   * Save from the close dialog. Only a successful write closes the window;
   * cancelling a dialog, a blocked form or a failed write leaves it open.
   */
  const handleCloseSave = useCallback(async () => {
    if (closeSaveInFlightRef.current) {
      return;
    }
    closeSaveInFlightRef.current = true;
    setClosePrompt({ busy: true, message: null });
    try {
      const result = await saveRef.current.saveForClose();
      if (result === "saved") {
        await closeWindow();
        return;
      }
      if (closingRef.current) {
        return;
      }
      setClosePrompt({
        busy: false,
        message:
          result === "failed"
            ? "保存失败，窗口未关闭。请取消后重试保存。"
            : result === "blocked"
              ? "有必填字段尚未填写，窗口未关闭。请取消后补全再试。"
              : null,
      });
    } finally {
      closeSaveInFlightRef.current = false;
    }
  }, [closeWindow]);

  const handleCloseDiscard = useCallback(() => {
    void closeWindow();
  }, [closeWindow]);

  const handleCloseCancel = useCallback(() => {
    setClosePrompt(null);
  }, []);

  useEffect(() => {
    setFormFocusFieldId(null);
  }, [openDocumentView?.documentId]);

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

  async function openPath(
    path: string,
    password: string | null = null,
    options: { fromRecentFile?: boolean } = {},
  ) {
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
          void recent.refresh();
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
      setEditor({ status: "open", document });
      void recent.refresh();
    } catch (error) {
      if (openedDocumentId) {
        await closeDocument(openedDocumentId).catch(() => {});
      }

      const commandError = toCommandError(error);
      if (
        options.fromRecentFile &&
        commandError.code === "file_not_found"
      ) {
        // A recent-files click that hits a moved or deleted file returns to
        // the start screen, explains why, and drops the stale entry.
        setEditor(previous);
        setPasswordPrompt(null);
        setNotice(commandError);
        recent.forget(path);
        return;
      }
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

  async function restoreRecoveryCandidate(sessionId: string) {
    const document = await recovery.restore(sessionId);
    if (!document) {
      return;
    }
    // The snapshot already contains the unsaved edits, so no password prompt
    // or loading workspace is needed; the source path stays provenance only.
    setNotice(null);
    setPasswordPrompt(null);
    setEditor({ status: "open", document });
    // Restoring records the original Source Document as recently opened.
    void recent.refresh();
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
    // Native menus do not fire disabled items, but an event can arrive while a
    // state push is in flight. The current availability is the same snapshot
    // the menu was built from, so a disabled command is ignored here too.
    if (!menuEnabledRef.current.get(menuId)) {
      return;
    }
    // The disabled snapshot reaches the native menu one command round trip
    // later. If an accelerator event arrives in that gap, the focused control
    // still owns the key; never let stale menu availability move the page or
    // undo the document out from under it.
    if (
      TEXT_OWNED_MENU_IDS.has(menuId) &&
      isEditableTarget(document.activeElement)
    ) {
      return;
    }
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
      case "edit.undo":
        void runHistoryAction("undo");
        break;
      case "edit.redo":
        void runHistoryAction("redo");
        break;
      case "page.prev":
        current.goToPage(current.pageIndex - 1);
        break;
      case "page.next":
        current.goToPage(current.pageIndex + 1);
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
          if (pdf && !closePromptRef.current && !recoveryPromptOpenRef.current) {
            void openPath(pdf);
          }
        }),
      );
      subscribe(
        onWindowCloseRequested((event) => {
          void handleCloseRequested(event);
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
          {open && save.dirty && (
            <span className="dirty-indicator" title="有未保存的更改">
              未保存
            </span>
          )}
          {open?.document.recovered && (
            <span
              className="recovered-indicator"
              title="此文档从恢复文件打开，原文件未被修改。"
            >
              已恢复
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

      {open && save.requiredFields && save.requiredFields.length > 0 && (
        <div className="app-notice app-notice--error" role="alert">
          <span>
            有 {save.requiredFields.length} 个必填字段尚未填写，无法保存：
            {save.requiredFields
              .slice(0, 3)
              .map((field) => `第 ${field.pageIndex + 1} 页「${field.name}」`)
              .join("、")}
            {save.requiredFields.length > 3 ? " 等" : ""}。已定位到第一个未填字段。
          </span>
          <button
            type="button"
            onClick={() => {
              const first = save.requiredFields?.[0];
              if (first) {
                locateField(first);
              }
            }}
          >
            定位第一个未填字段
          </button>
          <button type="button" onClick={save.dismissRequired}>
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

      {closePrompt && open && (
        <CloseConfirmDialog
          documentName={open.document.name}
          busy={closePrompt.busy || save.busy}
          message={closePrompt.message}
          onSave={() => void handleCloseSave()}
          onDiscard={handleCloseDiscard}
          onCancel={handleCloseCancel}
        />
      )}

      {/* Rendered after the close dialog so the unencrypted-output question it
          raises during the close save paints on top. */}
      {open && save.confirmation && (
        <SaveConfirmDialog
          documentName={open.document.name}
          busy={save.busy}
          onConfirm={save.confirmUnencrypted}
          onCancel={save.cancelConfirmation}
        />
      )}

      {open && pageDialog?.kind === "delete" && (
        <DeletePageDialog
          pageNumber={pageDialog.pageIndex + 1}
          pageCount={open.document.pageCount}
          onConfirm={confirmDeletePage}
          onCancel={() => setPageDialog(null)}
        />
      )}

      {open && pageDialog?.kind === "insert" && (
        <InsertBlankPageDialog
          afterPageNumber={pageDialog.afterIndex}
          pageCount={open.document.pageCount}
          currentPageSize={pageDialog.currentSize}
          onInsert={confirmInsertBlankPage}
          onCancel={() => setPageDialog(null)}
        />
      )}

      {recoveryPromptOpen && (
        <RecoveryPrompt
          candidates={recovery.candidates}
          busySessionId={recovery.busySessionId}
          error={recovery.error ? errorMessage(recovery.error.code) : null}
          onRestore={(sessionId) => void restoreRecoveryCandidate(sessionId)}
          onDiscard={recovery.discard}
          onDismiss={recovery.dismiss}
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
            {recent.files.length > 0 && (
              <div className="recent-files">
                <h3>最近文件</h3>
                <ul>
                  {recent.files.map((file) => (
                    <li key={file.path}>
                      <button
                        type="button"
                        className="recent-file"
                        aria-label={`打开 ${file.name}`}
                        title={file.path}
                        onClick={() => {
                          void openPath(file.path, null, {
                            fromRecentFile: true,
                          });
                        }}
                      >
                        <span className="recent-file-name">{file.name}</span>
                        <span className="recent-file-path">{file.path}</span>
                      </button>
                    </li>
                  ))}
                </ul>
              </div>
            )}
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
              canModifyPages={canModifyPages}
              pageOperationDisabledReason={pageOperationDisabledReason}
              onRotatePage={rotateCurrentPage}
              onDeletePage={openDeletePageDialog}
              onInsertPage={openInsertBlankPageDialog}
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
                canReorder={canModifyPages}
                onMovePage={moveThumbnailPage}
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
                    canAnnotate={open.document.permissions.canAnnotate}
                    annotations={annotations.annotations}
                    selectedAnnotation={selectedAnnotation}
                    armedStamp={armedStamp}
                    onSelectAnnotation={selectAnnotation}
                    onStampPlaced={handleStampPlaced}
                    onArmedStampChange={setArmedStamp}
                    onAnnotationsChanged={() => void annotations.reload()}
                    surfaceRef={viewer.surfaceRef}
                    canFillForms={
                      open.document.permissions.canFillForms &&
                      !open.document.readOnly
                    }
                    canModifyContent={
                      open.document.permissions.canModifyContent &&
                      !open.document.readOnly
                    }
                    formFocusFieldId={formFocusFieldId}
                    onFormFocusHandled={handleFormFocusHandled}
                    onSelectPage={viewer.goToPage}
                    onFormFocusField={setFormFocusFieldId}
                  />
                </div>
              </div>
              <div className="annotation-column">
                <StampPanel
                  armed={armedStamp}
                  canAnnotate={open.document.permissions.canAnnotate}
                  readOnly={open.document.readOnly}
                  onPick={setArmedStamp}
                />
                <AnnotationPanel
                  documentId={open.document.documentId}
                  annotations={annotations.annotations}
                  selectedName={selectedAnnotation?.name ?? null}
                  loading={annotations.status === "loading"}
                  error={annotations.error?.message ?? null}
                  canAnnotate={open.document.permissions.canAnnotate}
                  readOnly={open.document.readOnly}
                  onSelect={selectAnnotation}
                  onChanged={() => void annotations.reload()}
                />
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
