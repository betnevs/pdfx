import { describe, expect, it } from "vitest";

import {
  MENU_ITEM_IDS,
  menuItemStates,
  type MenuStateInput,
} from "./menu";

function enabledIds(input: MenuStateInput): string[] {
  return menuItemStates(input)
    .filter((item) => item.enabled)
    .map((item) => item.id);
}

const openDocument: MenuStateInput = {
  hasDocument: true,
  readOnly: false,
  canPrint: true,
  canUndo: false,
  canRedo: false,
  saving: false,
  modalOpen: false,
  textInputFocused: false,
  pageIndex: 1,
  pageCount: 3,
};

describe("menu item states", () => {
  it("disables every document action while no document is open", () => {
    const enabled = enabledIds({
      ...openDocument,
      hasDocument: false,
      pageIndex: 0,
    });

    expect(enabled).toEqual([
      MENU_ITEM_IDS.open,
      MENU_ITEM_IDS.settings,
      MENU_ITEM_IDS.fullscreen,
    ]);
  });

  it("enables save, save as, printing, viewing and page navigation for an editable document", () => {
    const enabled = enabledIds(openDocument);

    expect(enabled).toEqual([
      MENU_ITEM_IDS.open,
      MENU_ITEM_IDS.print,
      MENU_ITEM_IDS.exportPage,
      MENU_ITEM_IDS.save,
      MENU_ITEM_IDS.saveAs,
      MENU_ITEM_IDS.settings,
      MENU_ITEM_IDS.actualSize,
      MENU_ITEM_IDS.zoomIn,
      MENU_ITEM_IDS.zoomOut,
      MENU_ITEM_IDS.fitWidth,
      MENU_ITEM_IDS.fitPage,
      MENU_ITEM_IDS.fullscreen,
      MENU_ITEM_IDS.previousPage,
      MENU_ITEM_IDS.nextPage,
    ]);
  });

  it("keeps history actions disabled until the edit session has a step", () => {
    const states = menuItemStates({ ...openDocument, canUndo: true });

    expect(enabledIds({ ...openDocument, canUndo: true })).toContain(
      MENU_ITEM_IDS.undo,
    );
    expect(
      states.find((item) => item.id === MENU_ITEM_IDS.redo)?.enabled,
    ).toBe(false);
    expect(
      menuItemStates({ ...openDocument, canRedo: true }).find(
        (item) => item.id === MENU_ITEM_IDS.redo,
      )?.enabled,
    ).toBe(true);
  });

  it("disables page actions at the first and last page", () => {
    expect(
      menuItemStates({ ...openDocument, pageIndex: 0 }).find(
        (item) => item.id === MENU_ITEM_IDS.previousPage,
      )?.enabled,
    ).toBe(false);
    expect(
      menuItemStates({ ...openDocument, pageIndex: 2 }).find(
        (item) => item.id === MENU_ITEM_IDS.nextPage,
      )?.enabled,
    ).toBe(false);
    expect(
      menuItemStates({ ...openDocument, pageIndex: 0, pageCount: 1 }).filter(
        (item) =>
          item.id === MENU_ITEM_IDS.previousPage ||
          item.id === MENU_ITEM_IDS.nextPage,
      ),
    ).toEqual([
      { id: MENU_ITEM_IDS.previousPage, enabled: false },
      { id: MENU_ITEM_IDS.nextPage, enabled: false },
    ]);
  });

  it("keeps save as available but disables editing on a read-only document", () => {
    const enabled = enabledIds({ ...openDocument, readOnly: true, canUndo: true });

    expect(enabled).not.toContain(MENU_ITEM_IDS.save);
    expect(enabled).toContain(MENU_ITEM_IDS.saveAs);
    expect(enabled).not.toContain(MENU_ITEM_IDS.undo);
    expect(enabled).toContain(MENU_ITEM_IDS.previousPage);
    expect(enabled).toContain(MENU_ITEM_IDS.nextPage);
  });

  it("leaves text editing keys to the focused input", () => {
    const enabled = enabledIds({
      ...openDocument,
      canUndo: true,
      canRedo: true,
      textInputFocused: true,
    });

    expect(enabled).not.toContain(MENU_ITEM_IDS.undo);
    expect(enabled).not.toContain(MENU_ITEM_IDS.redo);
    expect(enabled).not.toContain(MENU_ITEM_IDS.previousPage);
    expect(enabled).not.toContain(MENU_ITEM_IDS.nextPage);
    expect(enabled).toContain(MENU_ITEM_IDS.save);
    expect(enabled).toContain(MENU_ITEM_IDS.zoomIn);
  });

  it("disables document actions while a modal question is open", () => {
    const enabled = enabledIds({
      ...openDocument,
      canUndo: true,
      canRedo: true,
      modalOpen: true,
    });

    expect(enabled).toEqual([]);
  });

  it("disables save and history while a save is running", () => {
    const enabled = enabledIds({
      ...openDocument,
      canUndo: true,
      canRedo: true,
      saving: true,
    });

    expect(enabled).not.toContain(MENU_ITEM_IDS.save);
    expect(enabled).not.toContain(MENU_ITEM_IDS.saveAs);
    expect(enabled).not.toContain(MENU_ITEM_IDS.undo);
    expect(enabled).not.toContain(MENU_ITEM_IDS.redo);
    expect(enabled).toContain(MENU_ITEM_IDS.previousPage);
  });

  it("disables printing when permission restrictions forbid it", () => {
    const enabled = enabledIds({ ...openDocument, canPrint: false });

    expect(enabled).not.toContain(MENU_ITEM_IDS.print);
    expect(enabled).toContain(MENU_ITEM_IDS.exportPage);
  });
});
