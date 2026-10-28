/**
 * Native menu state owned by the workspace.
 *
 * The menu itself lives in Rust (see `src-tauri/src/menu.rs`); the workspace
 * owns the document, permission, history and page state that decides whether
 * an item can act. This module is the only mapping from that state to the
 * `enabled` flags the native menu applies, so the toolbar, shortcuts and menu
 * cannot disagree about availability.
 *
 * Items are disabled when they cannot act. This is also what releases their
 * accelerators in the menu layer: a disabled item must not swallow a key that
 * belongs to a focused text input or to normal page scrolling.
 */

/** Menu ids shared with `src-tauri/src/menu.rs`. Keep both lists in step. */
export const MENU_ITEM_IDS = {
  open: "file.open",
  print: "file.print",
  exportPage: "file.export_page",
  save: "file.save",
  saveAs: "file.save_as",
  settings: "app.settings",
  undo: "edit.undo",
  redo: "edit.redo",
  actualSize: "view.actual_size",
  zoomIn: "view.zoom_in",
  zoomOut: "view.zoom_out",
  fitWidth: "view.fit_width",
  fitPage: "view.fit_page",
  fullscreen: "view.fullscreen",
  previousPage: "page.prev",
  nextPage: "page.next",
} as const;

export type MenuItemId = (typeof MENU_ITEM_IDS)[keyof typeof MENU_ITEM_IDS];

/** One menu item's enabled flag in the snapshot pushed to the menu layer. */
export interface MenuItemEnabledState {
  id: MenuItemId;
  enabled: boolean;
}

/** Workspace state the native menu availability depends on. */
export interface MenuStateInput {
  hasDocument: boolean;
  /** Permission Restrictions deny Save Changes (Save As is still allowed). */
  readOnly: boolean;
  /** Permission Restrictions allow printing. */
  canPrint: boolean;
  canUndo: boolean;
  canRedo: boolean;
  /** A save is writing; saves and history edits must not start another one. */
  saving: boolean;
  /** A blocking question owns the window (close guard, password, warning). */
  modalOpen: boolean;
  /** An input, textarea, select or contentEditable currently has focus. */
  textInputFocused: boolean;
  pageIndex: number;
  pageCount: number;
}

/**
 * Computes the full enabled snapshot for the native menu.
 *
 * Ordering is stable so callers can compare snapshots cheaply and tests read
 * like the menu itself.
 */
export function menuItemStates(input: MenuStateInput): MenuItemEnabledState[] {
  const documentOpen = input.hasDocument && !input.modalOpen;
  const saveAllowed = documentOpen && !input.saving;
  const editsAllowed = saveAllowed && !input.readOnly;
  // Text editing owns Cmd/Ctrl+Z, undo/redo and the page keys while it has
  // focus; the Rust menu layer releases those accelerators for disabled items.
  const historyAllowed = editsAllowed && !input.textInputFocused;
  const pageNavigationAllowed = documentOpen && !input.textInputFocused;
  return [
    { id: MENU_ITEM_IDS.open, enabled: !input.modalOpen },
    { id: MENU_ITEM_IDS.print, enabled: documentOpen && input.canPrint },
    { id: MENU_ITEM_IDS.exportPage, enabled: documentOpen },
    { id: MENU_ITEM_IDS.save, enabled: editsAllowed },
    { id: MENU_ITEM_IDS.saveAs, enabled: saveAllowed },
    { id: MENU_ITEM_IDS.settings, enabled: !input.modalOpen },
    { id: MENU_ITEM_IDS.undo, enabled: historyAllowed && input.canUndo },
    { id: MENU_ITEM_IDS.redo, enabled: historyAllowed && input.canRedo },
    { id: MENU_ITEM_IDS.actualSize, enabled: documentOpen },
    { id: MENU_ITEM_IDS.zoomIn, enabled: documentOpen },
    { id: MENU_ITEM_IDS.zoomOut, enabled: documentOpen },
    { id: MENU_ITEM_IDS.fitWidth, enabled: documentOpen },
    { id: MENU_ITEM_IDS.fitPage, enabled: documentOpen },
    { id: MENU_ITEM_IDS.fullscreen, enabled: !input.modalOpen },
    {
      id: MENU_ITEM_IDS.previousPage,
      enabled: pageNavigationAllowed && input.pageIndex > 0,
    },
    {
      id: MENU_ITEM_IDS.nextPage,
      enabled:
        pageNavigationAllowed && input.pageIndex < input.pageCount - 1,
    },
  ];
}

/** Stable signature used to push a snapshot only when availability changes. */
export function menuStateSignature(states: MenuItemEnabledState[]): string {
  return states
    .map((state) => `${state.id}:${state.enabled ? "1" : "0"}`)
    .join("|");
}
