import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  EditHistoryView,
  LinkMutationView,
  LinkView,
  TextCharView,
  TextLayerView,
} from "../lib/backend";
import { PageTextLayer } from "./PageTextLayer";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const invokeMock = vi.mocked(invoke);

function makeChar(text: string, x: number, y: number, width: number): TextCharView {
  return {
    text,
    x,
    y,
    width,
    height: 20,
    originX: x,
    originY: y + 20,
    fontSize: 18,
    generated: false,
  };
}

const textLayer: TextLayerView = {
  pageIndex: 2,
  pageWidth: 612,
  pageHeight: 792,
  rotation: 0,
  cropX: 0,
  cropY: 0,
  hasText: true,
  text: "Hi the",
  chars: [
    makeChar("H", 100, 100, 12),
    makeChar("i", 112, 100, 6),
    makeChar(" ", 118, 100, 6),
    makeChar("t", 124, 100, 6),
    makeChar("h", 130, 100, 8),
    makeChar("e", 138, 100, 8),
  ],
};

const idleHistory: EditHistoryView = {
  canUndo: false,
  canRedo: false,
  revision: 0,
  savedRevision: 0,
  dirty: false,
};

const dirtyHistory: EditHistoryView = {
  canUndo: true,
  canRedo: false,
  revision: 1,
  savedRevision: 0,
  dirty: true,
};

const createdLink: LinkView = {
  id: "2:0",
  name: "pdfx-link-1",
  pageIndex: 2,
  target: { kind: "url", url: "https://example.com/docs" },
  unsupported: null,
  rects: [{ x: 100, y: 100, width: 12, height: 20 }],
};

function linkMutation(target: LinkView["target"]): LinkMutationView {
  return {
    link: { ...createdLink, target },
    pageIndex: 2,
    history: dirtyHistory,
  };
}

function mockCommands(
  overrides: Record<string, (args: Record<string, unknown>) => unknown> = {},
) {
  const handlers: Record<string, (args: Record<string, unknown>) => unknown> = {
    extract_text_layer: () => textLayer,
    edit_history: () => idleHistory,
    create_link: () => linkMutation(createdLink.target),
    ...overrides,
  };
  invokeMock.mockImplementation((command, args) => {
    const handler = handlers[command as string];
    if (!handler) {
      return Promise.reject(new Error(`unexpected command ${command as string}`));
    }
    return Promise.resolve(handler((args ?? {}) as Record<string, unknown>));
  });
}

const originalGetBoundingClientRect = Element.prototype.getBoundingClientRect;

beforeEach(() => {
  Element.prototype.getBoundingClientRect = () =>
    ({
      x: 0,
      y: 0,
      left: 0,
      top: 0,
      right: 612,
      bottom: 792,
      width: 612,
      height: 792,
      toJSON: () => ({}),
    }) as DOMRect;
});

afterEach(() => {
  invokeMock.mockReset();
  Element.prototype.getBoundingClientRect = originalGetBoundingClientRect;
});

async function renderLayer(
  props: Partial<Parameters<typeof PageTextLayer>[0]> = {},
) {
  render(
    <PageTextLayer
      documentId="doc-1"
      pageIndex={2}
      canAnnotate
      pageCount={10}
      {...props}
    />,
  );
  await screen.findByRole("region", { name: "文本层" });
}

function selectFirstCharacter() {
  const region = screen.getByRole("region", { name: "文本层" });
  fireEvent.pointerDown(region, {
    clientX: 102,
    clientY: 105,
    button: 0,
    pointerId: 1,
  });
  fireEvent.pointerUp(region, { clientX: 102, clientY: 105, pointerId: 1 });
}

describe("PageTextLayer link creation", () => {
  it("creates a URL link over the selected text and applies the returned history", async () => {
    let linkCreated = false;
    mockCommands({
      edit_history: () => (linkCreated ? dirtyHistory : idleHistory),
      create_link: () => {
        linkCreated = true;
        return linkMutation(createdLink.target);
      },
    });
    await renderLayer();
    selectFirstCharacter();

    fireEvent.click(screen.getByRole("button", { name: "链接" }));
    expect(screen.getByRole("dialog", { name: "添加链接" })).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("链接网址"), {
      target: { value: "https://example.com/docs" },
    });
    fireEvent.click(screen.getByRole("button", { name: "添加" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("create_link", {
        documentId: "doc-1",
        request: {
          pageIndex: 2,
          rects: [{ x: 100, y: 100, width: 12, height: 20 }],
          target: { kind: "url", url: "https://example.com/docs" },
        },
      });
    });
    // The selection and the dialog are consumed by the new link.
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(screen.queryByRole("button", { name: "高亮" })).not.toBeInTheDocument();
    // The unified history now reports the link as undoable.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "撤销" })).toBeEnabled();
    });
  });

  it("creates an internal page link from a one-based page number", async () => {
    mockCommands({ create_link: () => linkMutation({ kind: "page", pageIndex: 2 }) });
    await renderLayer();
    selectFirstCharacter();

    fireEvent.click(screen.getByRole("button", { name: "链接" }));
    fireEvent.click(screen.getByRole("radio", { name: "页面" }));
    fireEvent.change(screen.getByLabelText("目标页码"), {
      target: { value: "3" },
    });
    fireEvent.click(screen.getByRole("button", { name: "添加" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "create_link",
        expect.objectContaining({
          request: expect.objectContaining({
            pageIndex: 2,
            target: { kind: "page", pageIndex: 2 },
          }),
        }),
      );
    });
  });

  it("keeps the dialog open with the Chinese validation reason", async () => {
    mockCommands();
    await renderLayer();
    selectFirstCharacter();

    fireEvent.click(screen.getByRole("button", { name: "链接" }));
    fireEvent.change(screen.getByLabelText("链接网址"), {
      target: { value: "ftp://example.com" },
    });
    fireEvent.click(screen.getByRole("button", { name: "添加" }));

    expect(screen.getByRole("alert")).toHaveTextContent(
      "链接网址必须以 http://、https:// 或 mailto: 开头。",
    );
    expect(invokeMock).not.toHaveBeenCalledWith("create_link", expect.anything());
  });

  it("maps a backend link rejection to a Chinese reason", async () => {
    mockCommands({
      create_link: () => {
        throw { code: "invalid_argument", message: "the link URL is invalid" };
      },
    });
    await renderLayer();
    selectFirstCharacter();

    fireEvent.click(screen.getByRole("button", { name: "链接" }));
    fireEvent.change(screen.getByLabelText("链接网址"), {
      target: { value: "https://example.com" },
    });
    fireEvent.click(screen.getByRole("button", { name: "添加" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "链接目标无效，请检查网址或页码。",
    );
    expect(screen.getByRole("dialog", { name: "添加链接" })).toBeInTheDocument();
  });

  it("offers no link action without annotation permission", async () => {
    mockCommands();
    await renderLayer({ canAnnotate: false });
    selectFirstCharacter();

    expect(
      screen.queryByRole("button", { name: "链接" }),
    ).not.toBeInTheDocument();
    expect(screen.getByText("此文档不允许添加注释")).toBeInTheDocument();
  });

  it("offers no link action in a read-only document", async () => {
    mockCommands();
    await renderLayer({ readOnly: true });
    selectFirstCharacter();

    expect(
      screen.queryByRole("button", { name: "链接" }),
    ).not.toBeInTheDocument();
  });
});
