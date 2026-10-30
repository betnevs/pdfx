import { useCallback, useEffect, useRef, useState } from "react";
import type {
  KeyboardEvent as ReactKeyboardEvent,
  PointerEvent as ReactPointerEvent,
} from "react";

import { DocumentNotices, DocumentSaveControls } from "./components/DocumentAccess";
import { AnnotationPanel } from "./components/AnnotationPanel";
import { AttachmentPanel, attachmentErrorMessage } from "./components/AttachmentPanel";
import { LayerPanel } from "./components/LayerPanel";
import { StampPanel } from "./components/StampPanel";
import { WatermarkPanel } from "./components/WatermarkPanel";
import {
  DocumentToolbar,
  PageOperationsPanel,
  PageSurface,
  ThumbnailRail,
} from "./components/PageViewer";
import { DocumentNavigation, type NavigationTab } from "./components/DocumentNavigation";
import { TextStylePlaceholder } from "./components/TextStylePlaceholder";
import { ContinuousPages } from "./components/ContinuousPages";
import { WorkspaceIcon, type WorkspaceIconName } from "./components/WorkspaceIcon";
import {
  EditorToolsProvider,
  ToolActionChip,
  ToolChips,
  ToolPanelHost,
  useEditorTools,
  type ToolGroup,
} from "./components/ToolShelf";
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
import { SaveOptionsDialog } from "./components/SaveOptionsDialog";
import { SignatureWarningBanner } from "./components/SignatureWarningBanner";
import {
  choosePdfFile,
  closeDocument,
  copyPage,
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
import { flushPendingEdits, hasPendingEdits, hasDirtyPendingEdits } from "./lib/pendingEdits";
import { scaleToPercent } from "./lib/pageGeometry";
import { notifyDocumentChanged } from "./lib/documentEvents";
import { isEditableTarget, isWindowsPlatform } from "./lib/keyboard";
import {
  MENU_ITEM_IDS,
  menuItemStates,
  menuStateSignature,
} from "./lib/menu";
import { useAnnotations } from "./lib/useAnnotations";
import { useAttachments } from "./lib/useAttachments";
import { useLinks } from "./lib/useLinks";
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

/**
 * Inspectors available beside the page. Each one owns the right-hand panel;
 * the page overlay tools themselves stay active regardless of the selection.
 */
type WorkspacePanel =
  | "annotate"
  | "stamp"
  | "watermark"
  | "attachments"
  | "layers"
  | "pages"
  | "export";

const WORKSPACE_PANELS: ReadonlyArray<{
  id: WorkspacePanel;
  label: string;
}> = [
  { id: "annotate", label: "注释" },
  { id: "stamp", label: "图章" },
  { id: "watermark", label: "水印" },
  { id: "attachments", label: "附件" },
  { id: "layers", label: "图层" },
  { id: "pages", label: "页面" },
  { id: "export", label: "导出" },
];

/** The five edit-mode tool tabs, mirroring the reference editor. */
type EditorTab = "annotate" | "shapes" | "insert" | "text" | "forms";

const EDITOR_TABS: ReadonlyArray<{
  id: EditorTab;
  label: string;
  icon: WorkspaceIconName;
}> = [
  { id: "annotate", label: "注释", icon: "comment" },
  { id: "shapes", label: "形状", icon: "shape" },
  { id: "insert", label: "插入", icon: "insert" },
  { id: "text", label: "编辑文本", icon: "textEdit" },
  { id: "forms", label: "表单", icon: "form" },
];

const TAB_GROUPS: Record<EditorTab, ToolGroup> = {
  annotate: "annotate",
  shapes: "shapes",
  insert: "insert",
  text: "text",
  forms: "forms",
};

/** PDFX-only inspectors live in the overflow menu, not the tool tabs. */
const OVERFLOW_PANELS = WORKSPACE_PANELS.filter(
  (panel) => panel.id !== "annotate",
);

/**
 * A chrome button that owns no page tool but must disarm whatever tool is
 * active before it acts (opening a panel, panning, switching context).
 */
function EditorChromeButton({
  label,
  icon,
  pressed,
  disabled = false,
  onClick,
  className = "",
  disarm = true,
}: {
  label: string;
  icon: WorkspaceIconName;
  pressed?: boolean;
  disabled?: boolean;
  onClick: () => void;
  className?: string;
  disarm?: boolean;
}) {
  const tools = useEditorTools();
  return (
    <button
      type="button"
      className={`icon-button ${className}`.trim()}
      aria-label={label}
      title={label}
      aria-pressed={pressed}
      disabled={disabled}
      onClick={async () => {
        if (disarm && hasPendingEdits() && !(await flushPendingEdits())) return;
        if (disarm) tools?.closeActive();
        onClick();
      }}
    >
      <WorkspaceIcon name={icon} />
    </button>
  );
}

function EditorIdleTextPanel({show}: {show:boolean}) {
  const tools = useEditorTools();
  if (!show || tools?.activeKey) return null;
  return <div className="inspector-default"><h2>文本样式</h2><TextStylePlaceholder /></div>;
}

function EditorRailActions({canUndo, canRedo, onUndo, onRedo}: {canUndo:boolean;canRedo:boolean;onUndo:()=>void;onRedo:()=>void}) {
  const tools = useEditorTools();
  return <div className="rail-history">
    <button type="button" aria-label="样式" title="样式" aria-pressed={tools?.showProperties} onClick={tools?.toggleProperties}><WorkspaceIcon name="palette" /></button>
    <span className="toolbar-divider" />
    <button type="button" aria-label="撤销" title="撤销" disabled={!canUndo} onClick={onUndo}><WorkspaceIcon name="undo" /></button>
    <button type="button" aria-label="重做" title="重做" disabled={!canRedo} onClick={onRedo}><WorkspaceIcon name="redo" /></button>
  </div>;
}

function OverflowMenu({
  active,
  onSelect,
}: {
  active: WorkspacePanel;
  onSelect: (panel: WorkspacePanel) => void;
}) {
  const [open, setOpen] = useState(false);
  const tools = useEditorTools();

  useEffect(() => {
    if (!open) {
      return;
    }
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setOpen(false);
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open]);

  return (
    <div className="overflow-menu">
      <button
        type="button"
        className="icon-button"
        aria-label="更多工具"
        title="更多工具"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        <WorkspaceIcon name="more" />
      </button>
      {open && (
        <>
          <button
            type="button"
            className="menu-scrim"
            aria-label="关闭菜单"
            onClick={() => setOpen(false)}
          />
          <div className="overflow-menu-list" role="menu" aria-label="更多工具">
            {OVERFLOW_PANELS.map((panel) => (
              <button
                key={panel.id}
                type="button"
                role="menuitem"
                aria-current={active === panel.id ? "true" : undefined}
                onClick={() => {
                  tools?.closeActive();
                  onSelect(panel.id);
                  setOpen(false);
                }}
              >
                {panel.label}
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  );
}

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
  // The signature warning is dismissible for the session. A successful save
  // re-shows it in the "now invalid" state, as does opening another document.
  const [signatureWarningDismissed, setSignatureWarningDismissed] =
    useState(false);
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
  const viewer = useDocumentViewer(openDocumentView, handleViewerError, true);
  const viewerRef = useRef(viewer);
  viewerRef.current = viewer;

  const annotations = useAnnotations(openDocumentView?.documentId ?? null);
  const links = useLinks(openDocumentView?.documentId ?? null);
  const attachments = useAttachments(openDocumentView?.documentId ?? null);
  const [selectedAnnotation, setSelectedAnnotation] =
    useState<AnnotationView | null>(null);
  const [armedStamp, setArmedStamp] = useState<StampKind | null>(null);
  const [activePanel, setActivePanel] = useState<WorkspacePanel>("annotate");
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [navigationTab, setNavigationTab] = useState<NavigationTab>("thumbnails");
  const [thumbnailSize, setThumbnailSize] = useState(144);
  const [editorTab, setEditorTab] = useState<EditorTab>("text");
  const [panMode, setPanMode] = useState(false);
  const panDrag = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    left: number;
    top: number;
  } | null>(null);
  const toolGroup: ToolGroup =
    TAB_GROUPS[editorTab];

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
    setActivePanel("annotate");
    setSidebarOpen(true);
    setEditorTab("text");
    setPanMode(false);
  }, [openDocumentView?.documentId]);

  function selectAnnotation(annotation: AnnotationView | null) {
    setSelectedAnnotation(annotation);
    setArmedStamp(null);
    if (annotation) {
      setEditorTab("annotate");
      setActivePanel("annotate");
      viewer.goToPage(annotation.pageIndex);
    }
  }



  function handlePanPointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    if (!panMode || event.button !== 0) {
      return;
    }
    const viewport = viewer.viewportRef.current;
    if (!viewport) {
      return;
    }
    panDrag.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      left: viewport.scrollLeft,
      top: viewport.scrollTop,
    };
    event.currentTarget.setPointerCapture?.(event.pointerId);
  }

  function handlePanPointerMove(event: ReactPointerEvent<HTMLDivElement>) {
    const drag = panDrag.current;
    const viewport = viewer.viewportRef.current;
    if (!drag || drag.pointerId !== event.pointerId || !viewport) {
      return;
    }
    viewport.scrollLeft = drag.left - (event.clientX - drag.startX);
    viewport.scrollTop = drag.top - (event.clientY - drag.startY);
  }

  function handlePanPointerUp(event: ReactPointerEvent<HTMLDivElement>) {
    if (panDrag.current?.pointerId === event.pointerId) {
      panDrag.current = null;
      event.currentTarget.releasePointerCapture?.(event.pointerId);
    }
  }

  function handleStampPlaced(annotation: AnnotationView) {
    setEditorTab("annotate");
    setArmedStamp(null);
    setSelectedAnnotation(annotation);
    // The placed stamp becomes the selected annotation, whose property
    // controls live in the 注释 inspector tab.
    setActivePanel("annotate");
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

  // Dismissal lasts until another document is opened or a save completes: the
  // post-save warning announces that the signatures are now invalid.
  const signatureDocumentId = openDocumentView?.documentId ?? null;
  useEffect(() => {
    setSignatureWarningDismissed(false);
  }, [signatureDocumentId, save.saveGeneration]);

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

  // Copies the visible page and selects the duplicate, which the backend
  // inserts immediately after the source page.
  const copyCurrentPage = useCallback(() => {
    const current = editorRef.current;
    if (current.status !== "open") {
      return;
    }
    if (current.document.readOnly || !current.document.permissions.canModifyContent) {
      return;
    }
    const documentId = current.document.documentId;
    const pageIndex = viewerRef.current.pageIndex;
    void applyPageOperation(() => copyPage(documentId, pageIndex));
  }, [applyPageOperation]);

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
  // A dialog can unmount while one of its inputs still has focus. Removing the
  // focused element does not reliably fire focusout, so the event-tracked flag
  // can outlive the control; only an editable element that still has focus may
  // own the shortcuts.
  const textInputOwnsShortcuts =
    textInputFocused && isEditableTarget(document.activeElement);
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
    textInputFocused: textInputOwnsShortcuts,
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
      if (hasPendingEdits(documentId) && !(await flushPendingEdits(documentId))) return;
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
        // The backend does not report which pages an undo/redo touched, and a
        // watermark action can span every page, so invalidate all thumbnails.
        allPagesChanged: true,
      });
    } catch (error) {
      setNotice(toCommandError(error));
    } finally {
      historyActionInFlightRef.current = false;
    }
  }, []);

  /**
   * Publishes a watermark change. The panel already adopted the command's
   * history; thumbnails only need invalidation. An all-pages apply or a
   * delete spans pages, while a current-page apply refreshes just that page.
   */
  const handleWatermarkApplied = useCallback((allPages: boolean) => {
    const current = editorRef.current;
    if (current.status !== "open") {
      return;
    }
    notifyDocumentChanged({
      documentId: current.document.documentId,
      pageIndex: viewerRef.current.pageIndex,
      allPagesChanged: allPages,
      source: "watermark",
    });
  }, []);

  /**
   * Publishes a layer change. Visibility and deletion can alter every page's
   * pixels, so the page bitmaps and all thumbnail caches are stale; a rename
   * changes no content and therefore skips the repaint.
   */
  const handleLayerChanged = useCallback((repaint: boolean) => {
    if (!repaint) {
      return;
    }
    const current = editorRef.current;
    if (current.status !== "open") {
      return;
    }
    notifyDocumentChanged({
      documentId: current.document.documentId,
      pageIndex: viewerRef.current.pageIndex,
      allPagesChanged: true,
      source: "layer",
    });
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
      if (history && !history.dirty && !hasDirtyPendingEdits(documentId)) {
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
  const handleEditorTabKeyDown = (
    event: ReactKeyboardEvent<HTMLButtonElement>,
    id: EditorTab,
  ) => {
    const index = EDITOR_TABS.findIndex((tab) => tab.id === id);
    let nextIndex: number | null = null;
    if (event.key === "ArrowRight") {
      nextIndex = (index + 1) % EDITOR_TABS.length;
    } else if (event.key === "ArrowLeft") {
      nextIndex = (index - 1 + EDITOR_TABS.length) % EDITOR_TABS.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = EDITOR_TABS.length - 1;
    }
    if (nextIndex === null) {
      return;
    }
    event.preventDefault();
    setEditorTab(EDITOR_TABS[nextIndex].id);
    const tablist = event.currentTarget.closest('[role="tablist"]');
    tablist
      ?.querySelectorAll<HTMLButtonElement>('[role="tab"]')
      [nextIndex]?.focus();
  };

  return (
    <div className="app-shell">
      <header className="app-header">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true">
            P
          </span>
          <h1>PDFX</h1>
        </div>
        {open && (
          <div className="document-chip" title={open.document.path}>
            <WorkspaceIcon name="page" className="document-chip-icon" />
            <span className="document-name">{open.document.name}</span>
            {save.dirty && (
              <span className="dirty-indicator" title="有未保存的更改">
                未保存
              </span>
            )}
            {open.document.recovered && (
              <span
                className="recovered-indicator"
                title="此文档从恢复文件打开，原文件未被修改。"
              >
                已恢复
              </span>
            )}
          </div>
        )}
        <div className="header-actions">
          <button
            type="button"
            className="header-button header-button--ghost"
            onClick={promptOpen}
          >
            打开 PDF…
          </button>
          <button
            type="button"
            className="icon-button"
            aria-label="设置"
            title="设置"
            onClick={() => setSettingsOpen(true)}
          >
            <WorkspaceIcon name="settings" />
          </button>
        </div>
      </header>

      {open && <DocumentNotices document={open.document} save={save} />}

      {notice && (
        <div className="app-notice" role="alert">
          <span>{errorMessage(notice.code)}</span>
          <button type="button" onClick={() => setNotice(null)}>
            关闭
          </button>
        </div>
      )}

      {open?.document.signature && !signatureWarningDismissed && (
        <SignatureWarningBanner
          signature={open.document.signature}
          invalidated={save.signatureInvalidated}
          onDismiss={() => setSignatureWarningDismissed(true)}
        />
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

      {/* Rendered after the close dialog so the save options it raises during
          the close save paint on top. */}
      {open && save.confirmation && (
        <SaveOptionsDialog
          documentName={open.document.name}
          busy={save.busy}
          sourceEncrypted={open.document.encrypted}
          outputEncrypted={save.confirmation.outputEncrypted}
          signature={open.document.signature ?? null}
          initialProtect={save.confirmation.preferPassword}
          onSave={save.confirmSaveOptions}
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
                <div className="recent-files-heading">
                  <h3>最近文件</h3>
                  <button type="button" className="recent-file-remove" disabled={recent.busy}
                    onClick={recent.clear} title="仅清空打开记录，不删除 PDF 文件">
                    全部清空
                  </button>
                </div>
                {recent.error && <p role="alert">{recent.error}</p>}
                <ul>
                  {recent.files.map((file) => (
                    <li key={file.path}>
                      <button
                        type="button"
                        className="recent-file"
                        disabled={recent.busy}
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
                      <button type="button" className="recent-file-remove" disabled={recent.busy}
                        aria-label={`移除 ${file.name} 的记录`} title="从最近文件移除，不删除 PDF 文件"
                        onClick={() => recent.forget(file.path)}>
                        移除
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
            <EditorToolsProvider
              group={toolGroup}
              onToolActive={() => setPanMode(false)}
            >
            <div className="editor-bar">
              <div className="editor-mode-label" aria-label="编辑模式"><WorkspaceIcon name="textEdit" /><span>编辑</span></div>
                  <div className="editor-bar-actions editor-bar-actions--leading">
                    <EditorChromeButton
                      label="左侧面板"
                      disarm={false}
                      icon="panelLeft"
                      pressed={sidebarOpen}
                      onClick={() => setSidebarOpen((current) => !current)}
                    />
                    <EditorChromeButton
                      label={panMode ? "退出拖动" : "拖动页面"}
                      icon="hand"
                      pressed={panMode}
                      onClick={() => setPanMode((current) => !current)}
                    />
                  </div>
                  <div
                    className="editor-tabs"
                    role="tablist"
                    aria-label="编辑工具"
                  >
                    {EDITOR_TABS.map((tab) => (
                      <button
                        key={tab.id}
                        type="button"
                        role="tab"
                        id={`editor-tab-${tab.id}`}
                        className="editor-tab"
                        aria-selected={editorTab === tab.id}
                        aria-controls={`editor-tab-panel-${tab.id}`}
                        tabIndex={editorTab === tab.id ? 0 : -1}
                        onClick={async () => { if (hasPendingEdits() && !(await flushPendingEdits())) return; setEditorTab(tab.id); setActivePanel(tab.id === "insert" ? "stamp" : "annotate"); setPanMode(false); }}
                        onKeyDown={(event) =>
                          handleEditorTabKeyDown(event, tab.id)
                        }
                      >
                        <WorkspaceIcon name={tab.icon} />
                        <span>{tab.label}</span>
                      </button>
                    ))}
                  </div>

              <div className="editor-bar-spacer" />

              <div className="editor-bar-actions">
                <EditorChromeButton
                  label="注释列表"
                  icon="comment"
                  pressed={activePanel === "annotate"}
                  onClick={() => { setEditorTab("annotate"); setActivePanel("annotate"); }}
                />
                <OverflowMenu
                  active={activePanel}
                  onSelect={(panel) => setActivePanel(panel)}
                />
                <button
                  type="button"
                  className="icon-button"
                  aria-label={viewer.fullscreen ? "退出全屏" : "全屏"}
                  title={viewer.fullscreen ? "退出全屏" : "全屏"}
                  onClick={viewer.toggleFullscreen}
                >
                  <WorkspaceIcon
                    name={viewer.fullscreen ? "exitFullscreen" : "fullscreen"}
                  />
                </button>
              </div>
            </div>

            {(
              <div
                className="tool-rail"
                id={`editor-tab-panel-${editorTab}`}
                role="group"
                aria-label={`${
                  EDITOR_TABS.find((tab) => tab.id === editorTab)?.label ?? ""
                }工具`}
              >
                <ToolChips className="tool-rail-tools" label="编辑工具" />
                {editorTab !== "text" && <EditorRailActions canUndo={save.canUndo && !save.busy} canRedo={save.canRedo && !save.busy} onUndo={() => void runHistoryAction("undo")} onRedo={() => void runHistoryAction("redo")} />}
                {editorTab === "insert" && (
                  <>
                    <ToolActionChip
                      group="insert"
                      label="橡皮图章"
                      icon="stamp"
                      order={1}
                      onClick={() => setActivePanel("stamp")}
                    />
                    <ToolActionChip
                      group="insert"
                      label="文件附件"
                      icon="attachment"
                      order={3}
                      onClick={() => setActivePanel("attachments")}
                    />
                  </>
                )}
              </div>
            )}

            <div className={`document-body${sidebarOpen ? "" : " document-body--sidebar-hidden"}`}>
              <aside className="thumbnail-sidebar" aria-label="页面缩略图侧栏">
                <div className="sidebar-navigation" role="toolbar" aria-label="页面导航">
                  <button type="button" aria-label="缩略图" aria-pressed={navigationTab === "thumbnails"} title="缩略图" onClick={() => setNavigationTab("thumbnails")}><WorkspaceIcon name="pages" /></button>
                  <button type="button" aria-label="大纲" aria-pressed={navigationTab === "outline"} title="大纲" onClick={() => setNavigationTab("outline")}><WorkspaceIcon name="list" /></button>
                  <button type="button" aria-label="书签" aria-pressed={navigationTab === "bookmarks"} title="书签" onClick={() => setNavigationTab("bookmarks")}><WorkspaceIcon name="bookmark" /></button>
                  <button type="button" aria-label="页面操作" title="页面操作" onClick={() => setActivePanel("pages")}><WorkspaceIcon name="more" /></button>
                </div>
                <div className="thumbnail-navigation" hidden={navigationTab !== "thumbnails"}>
                <div className="thumbnail-size-control">
                  <button type="button" aria-label="缩小缩略图" onClick={() => setThumbnailSize(s => Math.max(96, s - 24))}><WorkspaceIcon name="minus" /></button>
                  <input type="range" min="96" max="216" step="12" value={thumbnailSize} aria-label="缩略图大小" onChange={e => setThumbnailSize(Number(e.target.value))} />
                  <button type="button" aria-label="放大缩略图" onClick={() => setThumbnailSize(s => Math.min(216, s + 24))}><WorkspaceIcon name="plus" /></button>
                </div>
                <ThumbnailRail
                  thumbnailSize={thumbnailSize}
                  documentId={open.document.documentId}
                  pageCount={open.document.pageCount}
                  currentPageIndex={viewer.pageIndex}
                  onSelectPage={viewer.goToPage}
                  canReorder={canModifyPages}
                  onMovePage={moveThumbnailPage}
                />
                </div>
                {navigationTab !== "thumbnails" && <DocumentNavigation key={open.document.documentId} document={open.document} tab={navigationTab} pageIndex={viewer.pageIndex} onSelectPage={viewer.goToPage} />}
              </aside>
              <div className="page-area">
                <div
                  ref={viewer.viewportRef}
                  className={`page-stage${panMode ? " page-stage--pan" : ""}`}
                  role="region"
                  aria-label="页面视口"
                  tabIndex={0}
                  onScroll={viewer.handleScroll}
                  onKeyDown={viewer.handleViewportKeyDown}
                  onPointerDown={handlePanPointerDown}
                  onPointerMove={handlePanPointerMove}
                  onPointerUp={handlePanPointerUp}
                  onPointerCancel={handlePanPointerUp}
                >
                    <div className="page-stage-inner">
                    <ContinuousPages document={open.document} viewer={viewer}>
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
                        links={links.links}
                        onLinksChanged={() => void links.reload()}
                        pageCount={open.document.pageCount}
                      />
                    </ContinuousPages>
                    </div>
                  </div>
                  <div className="floating-bar">
                    <DocumentToolbar
                      pageIndex={viewer.pageIndex}
                      pageCount={open.document.pageCount}
                      scale={viewer.scale}
                      zoomMode={viewer.zoomMode}
                      onSelectPage={viewer.goToPage}
                      onZoomTo={viewer.zoomTo}
                      onZoomIn={() => viewer.zoomBy(ZOOM_STEP)}
                      onZoomOut={() => viewer.zoomBy(1 / ZOOM_STEP)}
                      onFit={viewer.fit}
                      onActualSize={viewer.actualSize}
                      onSearch={() => {setSidebarOpen(true);setNavigationTab("search");}}
                    />
                  </div>
              </div>
              <aside className="inspector" aria-label="工具面板">
                <ToolPanelHost />
                <EditorIdleTextPanel show={activePanel === "annotate" && editorTab === "text"} />

                <div className="inspector-body">
                  <div
                    className="inspector-panel"
                    role="tabpanel"
                    id="workspace-panel-annotate"
                    aria-labelledby="workspace-tab-annotate"
                    hidden={activePanel !== "annotate" || editorTab === "text"}
                  >
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
                  <div
                    className="inspector-panel"
                    role="tabpanel"
                    id="workspace-panel-stamp"
                    aria-labelledby="workspace-tab-stamp"
                    hidden={activePanel !== "stamp"}
                  >
                    <StampPanel
                      armed={armedStamp}
                      canAnnotate={open.document.permissions.canAnnotate}
                      readOnly={open.document.readOnly}
                      onPick={setArmedStamp}
                    />
                  </div>
                  <div
                    className="inspector-panel"
                    role="tabpanel"
                    id="workspace-panel-watermark"
                    aria-labelledby="workspace-tab-watermark"
                    hidden={activePanel !== "watermark"}
                  >
                    <WatermarkPanel
                      documentId={open.document.documentId}
                      pageIndex={viewer.pageIndex}
                      readOnly={open.document.readOnly}
                      canModifyContent={
                        open.document.permissions.canModifyContent
                      }
                      onHistory={(history) => save.acceptHistory(history)}
                      onApplied={handleWatermarkApplied}
                    />
                  </div>
                  <div
                    className="inspector-panel"
                    role="tabpanel"
                    id="workspace-panel-attachments"
                    aria-labelledby="workspace-tab-attachments"
                    hidden={activePanel !== "attachments"}
                  >
                    <AttachmentPanel
                      documentId={open.document.documentId}
                      attachments={attachments.attachments}
                      loading={attachments.status === "loading"}
                      error={
                        attachments.error
                          ? attachmentErrorMessage(attachments.error.code)
                          : null
                      }
                      canModify={
                        open.document.permissions.canModifyContent &&
                        !open.document.readOnly
                      }
                      readOnly={open.document.readOnly}
                      onHistory={(history) => save.acceptHistory(history)}
                      onChanged={() => void attachments.reload()}
                    />
                  </div>
                  <div
                    className="inspector-panel"
                    role="tabpanel"
                    id="workspace-panel-layers"
                    aria-labelledby="workspace-tab-layers"
                    hidden={activePanel !== "layers"}
                  >
                    <LayerPanel
                      documentId={open.document.documentId}
                      canModifyContent={
                        open.document.permissions.canModifyContent
                      }
                      readOnly={open.document.readOnly}
                      onHistory={(history) => save.acceptHistory(history)}
                      onChanged={handleLayerChanged}
                    />
                  </div>
                  <div
                    className="inspector-panel"
                    role="tabpanel"
                    id="workspace-panel-pages"
                    aria-labelledby="workspace-tab-pages"
                    hidden={activePanel !== "pages"}
                  >
                    <PageOperationsPanel
                      pageCount={open.document.pageCount}
                      canModifyPages={canModifyPages}
                      pageOperationDisabledReason={pageOperationDisabledReason}
                      onRotatePage={rotateCurrentPage}
                      onCopyPage={copyCurrentPage}
                      onDeletePage={openDeletePageDialog}
                      onInsertPage={openInsertBlankPageDialog}
                    />
                  </div>
                  <div
                    className="inspector-panel"
                    role="tabpanel"
                    id="workspace-panel-export"
                    aria-labelledby="workspace-tab-export"
                    hidden={activePanel !== "export"}
                  >
                    <PrintExport
                      ref={printExportRef}
                      document={open.document}
                      pageIndex={viewer.pageIndex}
                    />
                  </div>
                </div>
                <div className="inspector-history" hidden={editorTab !== "text"} role="group" aria-label="编辑历史">
                  <button type="button" aria-label="撤销" title="撤销" disabled={!save.canUndo || save.busy} onClick={() => void runHistoryAction("undo")}><WorkspaceIcon name="undo" /></button>
                  <button type="button" aria-label="重做" title="重做" disabled={!save.canRedo || save.busy} onClick={() => void runHistoryAction("redo")}><WorkspaceIcon name="redo" /></button>
                </div>
                <div className="save-dock"><DocumentSaveControls document={open.document} save={save} /></div>
              </aside>
            </div>
            </EditorToolsProvider>
            <span className="visually-hidden">
              第 {viewer.pageIndex + 1} / {open.document.pageCount} 页 ·{" "}
              {scaleToPercent(viewer.scale)}%
            </span>
          </section>
        )}
      </main>

      <span className="visually-hidden" role="status">
        {statusText(appInfo)}
      </span>

      <SettingsPanel
        open={settingsOpen}
        settings={settings.settings}
        onClose={() => setSettingsOpen(false)}
        onSave={settings.update}
      />
    </div>
  );
}
