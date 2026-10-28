import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  notifyDocumentChanged,
} from "../lib/documentEvents";
import type {
  EditHistoryView,
  WatermarkEditResult,
  WatermarkImagePreview,
  WatermarkView,
} from "../lib/backend";
import { WatermarkPanel } from "./WatermarkPanel";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const openDialogMock = vi.mocked(openDialog);

const dirtyHistory: EditHistoryView = {
  canUndo: true,
  canRedo: false,
  revision: 1,
  savedRevision: 0,
  dirty: true,
};

const textView: WatermarkView = {
  kind: "text",
  scope: "current",
  text: "机密",
  imageName: null,
  imagePath: null,
  opacity: 40,
  rotation: -30,
  scale: 120,
  positionX: 0.4,
  positionY: 0.6,
  pageIndices: [0],
};

const imagePreview: WatermarkImagePreview = {
  name: "logo.png",
  mimeType: "image/png",
  dataUrl: "data:image/png;base64,iVBORw0KGgo=",
  width: 64,
  height: 32,
};

const imageView: WatermarkView = {
  kind: "image",
  scope: "all",
  text: null,
  imageName: "logo.png",
  imagePath: "/tmp/logo.png",
  opacity: 60,
  rotation: 15,
  scale: 80,
  positionX: 0.5,
  positionY: 0.5,
  pageIndices: [0, 1, 2],
};

function renderPanel(
  overrides: Partial<{
    documentId: string;
    pageIndex: number;
    readOnly: boolean;
    canModifyContent: boolean;
    onHistory: (history: EditHistoryView) => void;
    onApplied: (allPages: boolean) => void;
  }> = {},
) {
  const onHistory = overrides.onHistory ?? vi.fn();
  const onApplied = overrides.onApplied ?? vi.fn();
  const result = render(
    <WatermarkPanel
      documentId={overrides.documentId ?? "doc-1"}
      pageIndex={overrides.pageIndex ?? 2}
      readOnly={overrides.readOnly ?? false}
      canModifyContent={overrides.canModifyContent ?? true}
      onHistory={onHistory}
      onApplied={onApplied}
    />,
  );
  return { ...result, onHistory, onApplied };
}

function mockCommands(handlers: Record<string, (args?: unknown) => unknown>) {
  invokeMock.mockImplementation(async (command: string, args?: unknown) => {
    const handler = handlers[command];
    if (!handler) {
      throw new Error(`unexpected command: ${command}`);
    }
    return handler(args);
  });
}

describe("WatermarkPanel", () => {
  beforeEach(() => {
    mockCommands({ watermark_state: () => null });
  });

  afterEach(() => {
    invokeMock.mockReset();
    openDialogMock.mockReset();
    cleanup();
  });

  it("shows the Chinese text, image, adjustment and scope controls", async () => {
    renderPanel();

    expect(screen.getByRole("heading", { name: "水印" })).toBeInTheDocument();
    expect(screen.getByLabelText("文字")).toBeChecked();
    expect(screen.getByLabelText("图片")).not.toBeChecked();
    expect(screen.getByLabelText("水印文字")).toBeInTheDocument();
    expect(screen.getByLabelText("不透明度")).toHaveValue("35");
    expect(screen.getByLabelText("旋转")).toHaveValue("0");
    expect(screen.getByLabelText("缩放")).toHaveValue("100");
    expect(screen.getByLabelText("水平位置")).toHaveValue("50");
    expect(screen.getByLabelText("垂直位置")).toHaveValue("50");
    expect(screen.getByLabelText("当前页")).toBeChecked();
    expect(screen.getByLabelText("所有页面")).not.toBeChecked();
    expect(screen.getByRole("button", { name: "应用水印" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "删除水印" })).toBeDisabled();
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("watermark_state", {
        documentId: "doc-1",
      }),
    );
  });

  it("applies trimmed text with the adjusted opacity, rotation, scale and position", async () => {
    const applied: WatermarkEditResult = {
      watermark: {
        ...textView,
        opacity: 55,
        rotation: 30,
        scale: 120,
        positionX: 0.4,
        positionY: 0.6,
      },
      history: dirtyHistory,
    };
    mockCommands({
      watermark_state: () => null,
      apply_watermark: () => applied,
    });
    const { onHistory, onApplied } = renderPanel();

    fireEvent.change(screen.getByLabelText("水印文字"), {
      target: { value: "  机密  " },
    });
    fireEvent.change(screen.getByLabelText("不透明度"), {
      target: { value: "55" },
    });
    fireEvent.change(screen.getByLabelText("旋转"), {
      target: { value: "30" },
    });
    fireEvent.change(screen.getByLabelText("缩放"), {
      target: { value: "120" },
    });
    fireEvent.change(screen.getByLabelText("水平位置"), {
      target: { value: "40" },
    });
    fireEvent.change(screen.getByLabelText("垂直位置"), {
      target: { value: "60" },
    });
    fireEvent.click(screen.getByRole("button", { name: "应用水印" }));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("apply_watermark", {
        documentId: "doc-1",
        pageIndex: 2,
        request: {
          scope: "current",
          kind: "text",
          text: "机密",
          imagePath: null,
          opacity: 55,
          rotation: 30,
          scale: 120,
          positionX: 0.4,
          positionY: 0.6,
        },
      }),
    );
    expect(onHistory).toHaveBeenCalledWith(dirtyHistory);
    expect(onApplied).toHaveBeenCalledWith(false);
    expect(
      await screen.findByText("当前水印：文字「机密」（1 页）"),
    ).toBeInTheDocument();
    expect(screen.getByText("已应用到当前页。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "删除水印" })).toBeEnabled();
  });

  it("applies to all pages when that scope is selected", async () => {
    mockCommands({
      watermark_state: () => null,
      apply_watermark: () => ({
        watermark: { ...textView, scope: "all", pageIndices: [0, 1, 2] },
        history: dirtyHistory,
      }),
    });
    const { onApplied } = renderPanel();

    fireEvent.change(screen.getByLabelText("水印文字"), {
      target: { value: "全页" },
    });
    fireEvent.click(screen.getByLabelText("所有页面"));
    fireEvent.click(screen.getByRole("button", { name: "应用水印" }));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith(
        "apply_watermark",
        expect.objectContaining({
          request: expect.objectContaining({ scope: "all", text: "全页" }),
        }),
      ),
    );
    expect(onApplied).toHaveBeenCalledWith(true);
    expect(screen.getByText("已应用到所有可见页面。")).toBeInTheDocument();
  });

  it("rejects empty, multi-line and over-long text without calling the backend", async () => {
    renderPanel();
    const applyButton = screen.getByRole("button", { name: "应用水印" });

    fireEvent.click(applyButton);
    expect(await screen.findByRole("alert")).toHaveTextContent("请输入水印文字。");

    fireEvent.change(screen.getByLabelText("水印文字"), {
      target: { value: "   " },
    });
    fireEvent.click(applyButton);
    expect(screen.getByRole("alert")).toHaveTextContent("请输入水印文字。");

    fireEvent.change(screen.getByLabelText("水印文字"), {
      target: { value: "字".repeat(201) },
    });
    fireEvent.click(applyButton);
    expect(screen.getByRole("alert")).toHaveTextContent(
      "水印文字不能超过 200 个字符。",
    );

    fireEvent.click(screen.getByLabelText("图片"));
    fireEvent.click(applyButton);
    expect(screen.getByRole("alert")).toHaveTextContent("请先选择水印图片。");

    expect(
      invokeMock.mock.calls.filter(([command]) => command === "apply_watermark"),
    ).toHaveLength(0);
  });

  it("previews a chosen image and applies or deletes the image watermark", async () => {
    openDialogMock.mockResolvedValue("/tmp/logo.png");
    mockCommands({
      watermark_state: () => null,
      watermark_image_preview: () => imagePreview,
      apply_watermark: () => ({ watermark: imageView, history: dirtyHistory }),
      delete_watermark: () => ({
        watermark: null,
        history: { ...dirtyHistory, revision: 2 },
      }),
    });
    const { container, onHistory, onApplied } = renderPanel();

    fireEvent.click(screen.getByLabelText("图片"));
    fireEvent.click(screen.getByRole("button", { name: "选择图片…" }));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("watermark_image_preview", {
        path: "/tmp/logo.png",
      }),
    );
    expect(
      screen.getByText("logo.png · 64×32 px"),
    ).toBeInTheDocument();
    const preview = container.querySelector(".watermark-preview-image");
    expect(preview).toHaveAttribute("src", imagePreview.dataUrl);

    fireEvent.click(screen.getByRole("button", { name: "应用水印" }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("apply_watermark", {
        documentId: "doc-1",
        pageIndex: 2,
        request: {
          scope: "current",
          kind: "image",
          text: null,
          imagePath: "/tmp/logo.png",
          opacity: 35,
          rotation: 0,
          scale: 100,
          positionX: 0.5,
          positionY: 0.5,
        },
      }),
    );
    expect(onApplied).toHaveBeenCalledWith(false);
    expect(
      await screen.findByText("当前水印：图片 logo.png（3 页）"),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "删除水印" }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("delete_watermark", {
        documentId: "doc-1",
      }),
    );
    expect(onHistory).toHaveBeenLastCalledWith({
      ...dirtyHistory,
      revision: 2,
    });
    expect(onApplied).toHaveBeenLastCalledWith(true);
    expect(screen.getByText("已删除水印。")).toBeInTheDocument();
    expect(
      screen.queryByText("当前水印：图片 logo.png（3 页）"),
    ).not.toBeInTheDocument();
  });

  it("refreshes the form and preview after an undo-style all-pages change", async () => {
    let current: WatermarkView | null = {
      ...textView,
      opacity: 40,
      pageIndices: [0],
    };
    mockCommands({
      watermark_state: () => current,
      watermark_image_preview: () => imagePreview,
    });
    renderPanel();

    await waitFor(() =>
      expect(screen.getByLabelText("水印文字")).toHaveValue("机密"),
    );
    expect(screen.getByLabelText("不透明度")).toHaveValue("40");
    expect(screen.getByLabelText("当前页")).toBeChecked();
    expect(screen.getByText("当前水印：文字「机密」（1 页）")).toBeInTheDocument();

    current = {
      ...imageView,
      pageIndices: [0, 1, 2],
    };
    notifyDocumentChanged({
      documentId: "doc-1",
      pageIndex: 0,
      allPagesChanged: true,
    });

    expect(
      await screen.findByText("当前水印：图片 logo.png（3 页）"),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("图片")).toBeChecked();
    expect(screen.getByLabelText("所有页面")).toBeChecked();
    expect(screen.getByLabelText("不透明度")).toHaveValue("60");
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("watermark_image_preview", {
        path: "/tmp/logo.png",
      }),
    );
  });

  it("explains read-only and restricted documents in Chinese", () => {
    renderPanel({ readOnly: true });
    expect(
      screen.getByText("文档为只读，无法修改水印。"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "应用水印" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "删除水印" })).toBeDisabled();
    expect(screen.getByLabelText("水印文字")).toBeDisabled();
    expect(screen.getByLabelText("所有页面")).toBeDisabled();

    cleanup();
    mockCommands({ watermark_state: () => null });
    renderPanel({ canModifyContent: false });
    expect(
      screen.getByText("当前文档的权限不允许修改水印。"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "应用水印" })).toBeDisabled();
  });

  it("shows a Chinese permission error when applying is refused", async () => {
    mockCommands({
      watermark_state: () => null,
      apply_watermark: () => {
        throw { code: "permission_denied", message: "denied" };
      },
    });
    renderPanel();

    fireEvent.change(screen.getByLabelText("水印文字"), {
      target: { value: "DENIED" },
    });
    fireEvent.click(screen.getByRole("button", { name: "应用水印" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "当前文档的权限不允许修改水印。",
    );
    expect(
      screen.queryByText("当前水印：文字「DENIED」（1 页）"),
    ).not.toBeInTheDocument();
  });
});
