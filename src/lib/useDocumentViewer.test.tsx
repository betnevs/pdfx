import { act, renderHook, render, screen, fireEvent, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";

import type { DocumentView } from "./backend";
import { notifyDocumentChanged } from "./documentEvents";
import { renderPixelRatio, useDocumentViewer } from "./useDocumentViewer";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const getCurrentWindowMock = vi.mocked(getCurrentWindow);

const document: DocumentView = {
  documentId: "doc-1",
  name: "sample.pdf",
  path: "/tmp/sample.pdf",
  pageCount: 1,
  pageWidth: 612,
  pageHeight: 792,
  encrypted: false,
  readOnly: false,
  permissions: {
    canModifyContent: true,
    canAnnotate: true,
    canAssemble: true,
    canFillForms: true,
    canPrint: true,
    canCopyContent: true,
  },
};

function renderPageCalls() {
  return invokeMock.mock.calls.filter(([command]) => command === "render_page");
}

beforeEach(() => {
  getCurrentWindowMock.mockReturnValue({
    isFullscreen: vi.fn().mockResolvedValue(false),
    setFullscreen: vi.fn().mockResolvedValue(undefined),
    setTitle: vi.fn().mockResolvedValue(undefined),
  } as unknown as ReturnType<typeof getCurrentWindow>);

  invokeMock.mockImplementation((command) => {
    switch (command) {
      case "page_info":
        return Promise.resolve({
          pageIndex: 0,
          width: 612,
          height: 792,
          rotation: 0,
          cropX: 0,
          cropY: 0,
        });
      case "render_page":
        return Promise.resolve({
          documentId: "doc-1",
          pageIndex: 0,
          scale: 1,
          width: 612,
          height: 792,
          pngBase64: "",
        });
      default:
        return Promise.reject(new Error(`unexpected command ${command as string}`));
    }
  });
});

afterEach(() => {
  invokeMock.mockReset();
  getCurrentWindowMock.mockReset();
});

describe("renderPixelRatio", () => {
  it("renders at least one bitmap pixel per CSS pixel", () => {
    expect(renderPixelRatio(1, 1)).toBe(1);
    expect(renderPixelRatio(1, 0)).toBe(1);
  });

  it("renders Retina screens at twice the density", () => {
    expect(renderPixelRatio(1, 2)).toBe(2);
    expect(renderPixelRatio(1.5, 3)).toBe(2);
  });

  it("keeps the backend scale under its 40x limit", () => {
    expect(renderPixelRatio(30, 2)).toBeCloseTo(40 / 30, 5);
    expect(renderPixelRatio(40, 2)).toBe(1);
    expect(renderPixelRatio(80, 2)).toBe(1);
  });
});

describe("useDocumentViewer", () => {
  it("re-renders the page after its content changes", async () => {
    renderHook(() => useDocumentViewer(document, vi.fn()));

    await waitFor(() => expect(renderPageCalls()).toHaveLength(1));

    notifyDocumentChanged({ documentId: "doc-1", pageIndex: 0 });

    await waitFor(() => expect(renderPageCalls()).toHaveLength(2));
  });

  it("ignores content changes in another document", async () => {
    renderHook(() => useDocumentViewer(document, vi.fn()));
    await waitFor(() => expect(renderPageCalls()).toHaveLength(1));

    notifyDocumentChanged({ documentId: "doc-2", pageIndex: 0 });
    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(renderPageCalls()).toHaveLength(1);
  });

  it("renders pages at the display density so they stay sharp", async () => {
    const density = vi
      .spyOn(window, "devicePixelRatio", "get")
      .mockReturnValue(2);
    try {
      const { result } = renderHook(() => useDocumentViewer(document, vi.fn()));
      await waitFor(() => expect(renderPageCalls()).toHaveLength(1));
      expect(renderPageCalls()[0][1]).toMatchObject({ scale: 2 });

      act(() => result.current.zoomTo(2));
      await waitFor(() =>
        expect(renderPageCalls().at(-1)?.[1]).toMatchObject({ scale: 4 }),
      );
    } finally {
      density.mockRestore();
    }
  });

  it("leaves Home and End to a focused text control", async () => {
    const multiPage = { ...document, pageCount: 3 };
    const { result } = renderHook(() =>
      useDocumentViewer(multiPage, vi.fn()),
    );
    await waitFor(() => expect(result.current.pageIndex).toBe(0));

    const input = window.document.createElement("input");
    window.document.body.appendChild(input);
    const preventDefault = vi.fn();

    act(() => {
      result.current.handleViewportKeyDown({
        key: "End",
        metaKey: false,
        ctrlKey: false,
        altKey: false,
        target: input,
        defaultPrevented: false,
        preventDefault,
      } as unknown as ReactKeyboardEvent<HTMLElement>);
    });

    expect(result.current.pageIndex).toBe(0);
    expect(preventDefault).not.toHaveBeenCalled();
    input.remove();
  });
});


describe("document opening and wheel zoom", () => {
  function Canvas({continuous=false}:{continuous?:boolean}) {
    const viewer=useDocumentViewer(document,vi.fn(),continuous);
    return <><div ref={viewer.viewportRef} data-testid="wheel-stage"><div className="page-stage-inner" style={{padding:24}}><div ref={viewer.surfaceRef} data-testid="wheel-page" /></div></div><output data-testid="wheel-scale">{viewer.scale}</output><output data-testid="wheel-mode">{viewer.zoomMode}</output></>;
  }
  it("opens the continuous workspace at the available page width", async () => {
    render(<Canvas continuous />);
    const stage=screen.getByTestId("wheel-stage");
    Object.defineProperties(stage,{clientWidth:{value:1200,configurable:true},clientHeight:{value:800,configurable:true}});
    fireEvent(window,new Event("resize"));
    await waitFor(()=>expect(Number(screen.getByTestId("wheel-scale").textContent)).toBeCloseTo(1152/612,4));
    expect(screen.getByTestId("wheel-mode")).toHaveTextContent("width");
  });
  it("zooms Ctrl/Command-wheel around the pointer and leaves ordinary scrolling alone", async () => {
    render(<Canvas />);
    const stage=screen.getByTestId("wheel-stage"),page=screen.getByTestId("wheel-page");
    Object.defineProperties(stage,{clientWidth:{value:800,configurable:true},clientHeight:{value:600,configurable:true}});
    Object.defineProperties(page,{offsetLeft:{value:100,configurable:true},offsetTop:{value:100,configurable:true}});
    vi.spyOn(stage,"getBoundingClientRect").mockReturnValue({x:20,y:40,left:20,top:40,right:820,bottom:640,width:800,height:600,toJSON:()=>({})});
    const zoom=new WheelEvent("wheel",{bubbles:true,cancelable:true,ctrlKey:true,deltaY:-100,clientX:300,clientY:250});
    act(()=>{page.dispatchEvent(zoom);});
    expect(zoom.defaultPrevented).toBe(true);
    await waitFor(()=>expect(Number(screen.getByTestId("wheel-scale").textContent)).toBeGreaterThan(1));
    const scale=Number(screen.getByTestId("wheel-scale").textContent);
    expect(100-stage.scrollLeft+180*scale).toBeCloseTo(280,3);
    expect(100-stage.scrollTop+110*scale).toBeCloseTo(210,3);
    const scroll=new WheelEvent("wheel",{bubbles:true,cancelable:true,deltaY:100});
    act(()=>{page.dispatchEvent(scroll);});
    expect(scroll.defaultPrevented).toBe(false);
    expect(Number(screen.getByTestId("wheel-scale").textContent)).toBe(scale);
    act(()=>{page.dispatchEvent(new WheelEvent("wheel",{bubbles:true,cancelable:true,metaKey:true,deltaY:100,clientX:300,clientY:250}));});
    await waitFor(()=>expect(Number(screen.getByTestId("wheel-scale").textContent)).toBeCloseTo(1,3));
  });
});

it("keeps the last bitmap visible while repeated zoom renders are pending", async () => {
  const { result } = renderHook(() => useDocumentViewer(document, vi.fn()));
  await waitFor(() => expect(result.current.render.image).not.toBeNull());
  const original = result.current.render.image;
  invokeMock.mockImplementation((command) => command === "render_page"
    ? new Promise(() => {})
    : Promise.reject(new Error(`unexpected ${command}`)));
  act(() => result.current.zoomTo(1.2));
  await waitFor(() => expect(renderPageCalls().length).toBeGreaterThan(1));
  expect(result.current.render.image).toBe(original);
  act(() => result.current.zoomTo(1.5));
  expect(result.current.render.image).toBe(original);
});
