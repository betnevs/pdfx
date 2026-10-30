import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { textBoxEditorFonts, type TextBoxTargetView } from "./backend";
import { useTextEditorFont } from "./useTextEditorFont";
vi.mock("./backend", () => ({textBoxEditorFonts: vi.fn()}));
const fetchFonts = vi.mocked(textBoxEditorFonts);
const add = vi.fn(), remove = vi.fn();
const originalFonts = Object.getOwnPropertyDescriptor(document, "fonts");
const target = {boxId: 7, keepOriginalFont: true, fontFamily: "FangS-SC", bold: false, italic: false} as TextBoxTargetView;
beforeEach(() => {
  vi.stubGlobal("FontFace", class {
    constructor(public family: string) {}
    load() { return Promise.resolve(this); }
  });
  Object.defineProperty(document, "fonts", {value: {add, delete: remove}, configurable: true});
});
afterEach(() => {
  cleanup(); vi.unstubAllGlobals(); vi.clearAllMocks();
  if (originalFonts) Object.defineProperty(document, "fonts", originalFonts);
});
it("uses the returned native font and releases it when the editor closes", async () => {
  fetchFonts.mockResolvedValue(["AAECAw=="]);
  const {result, rerender} = renderHook(({open}) => useTextEditorFont("doc", 0, open ? target : null, "项目经理  不错"), {initialProps: {open: true}});
  await waitFor(() => expect(result.current).toContain("PDFXEditor"));
  expect(fetchFonts).toHaveBeenCalledWith("doc", 0, 7, "项目经理  不错");
  expect(add).toHaveBeenCalledTimes(1);
  rerender({open: false});
  expect(result.current).not.toContain("PDFXEditor");
  expect(remove).toHaveBeenCalledWith(add.mock.calls[0][0]);
});
it("does not install a stale font after the text editor closes", async () => {
  let finish!: (value: string[]) => void;
  fetchFonts.mockReturnValue(new Promise(resolve => {finish = resolve;}));
  const {unmount} = renderHook(() => useTextEditorFont("doc", 0, target, "项目经理 不错"));
  await waitFor(() => expect(fetchFonts).toHaveBeenCalled());
  unmount();
  await act(async () => {finish(["AAECAw=="]);});
  expect(add).not.toHaveBeenCalled();
});
