import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  EditHistoryView,
  ImageEditResultView,
  ImageObjectView,
} from "../lib/backend";
import { insertSignatureImage } from "../lib/backend";
import { SIGNATURE_MAX_NAME_CHARS } from "../lib/signature";
import { SignatureDialog } from "./SignatureDialog";

vi.mock("../lib/backend", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/backend")>();
  return {
    ...actual,
    insertSignatureImage: vi.fn(),
  };
});

const insertSignatureImageMock = vi.mocked(insertSignatureImage);

const idleHistory: EditHistoryView = {
  canUndo: false,
  canRedo: false,
  revision: 0,
  savedRevision: 0,
  dirty: false,
};

const signatureImage: ImageObjectView = {
  imageId: 5,
  pageIndex: 0,
  name: "手绘签名",
  kind: "signature",
  x: 100,
  y: 200,
  width: 150,
  height: 60,
  rotation: 0,
  pixelWidth: 1600,
  pixelHeight: 600,
};

function inserted(image: ImageObjectView = signatureImage): ImageEditResultView {
  return { pageIndex: 0, image, history: { ...idleHistory, revision: 1 } };
}

class FakeContext {
  calls: string[] = [];
  font = "";
  strokeStyle = "";
  fillStyle = "";
  lineWidth = 1;
  lineCap = "";
  lineJoin = "";
  textAlign = "";
  textBaseline = "";

  clearRect(): void {
    this.calls.push("clearRect");
  }
  beginPath(): void {}
  moveTo(): void {}
  lineTo(): void {}
  stroke(): void {
    this.calls.push("stroke");
  }
  arc(): void {}
  fill(): void {}
  fillRect(): void {
    this.calls.push("fillRect");
  }
  translate(): void {}
  fillText(text: string): void {
    this.calls.push(`fillText:${text}`);
  }
  measureText(text: string): { width: number } {
    const size = Number.parseFloat(this.font) || 16;
    return { width: [...text].length * size };
  }
  drawImage(): void {
    this.calls.push("drawImage");
  }
  getImageData(_x: number, _y: number, width: number, height: number) {
    const data = new Uint8ClampedArray(width * height * 4);
    for (let offset = 0; offset < data.length; offset += 4) {
      data[offset] = 255;
      data[offset + 1] = 255;
      data[offset + 2] = 255;
      data[offset + 3] = 255;
    }
    // One inked pixel so background removal leaves something visible.
    data.set([20, 20, 20, 255], (Math.min(1, width - 1) + Math.min(1, height - 1) * width) * 4);
    return { width, height, data };
  }
  createImageData(width: number, height: number) {
    return { width, height, data: new Uint8ClampedArray(width * height * 4) };
  }
  putImageData(): void {
    this.calls.push("putImageData");
  }
}

class FakeImage {
  naturalWidth = 4;
  naturalHeight = 4;
  width = 4;
  height = 4;
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  set src(_value: string) {
    queueMicrotask(() => this.onload?.());
  }
}

const originalGetContext = HTMLCanvasElement.prototype.getContext;
const originalToDataURL = HTMLCanvasElement.prototype.toDataURL;
const originalGetBoundingClientRect = HTMLCanvasElement.prototype.getBoundingClientRect;
const originalCreateObjectUrl = URL.createObjectURL;
const originalRevokeObjectUrl = URL.revokeObjectURL;

beforeEach(() => {
  insertSignatureImageMock.mockReset();
  insertSignatureImageMock.mockResolvedValue(inserted());
  HTMLCanvasElement.prototype.getContext = function (
    this: HTMLCanvasElement,
    kind: string,
  ) {
    if (kind !== "2d") {
      return null;
    }
    return new FakeContext();
  } as unknown as typeof HTMLCanvasElement.prototype.getContext;
  HTMLCanvasElement.prototype.toDataURL = () => "data:image/png;base64,QUJD";
  HTMLCanvasElement.prototype.getBoundingClientRect = () =>
    ({
      x: 0,
      y: 0,
      left: 0,
      top: 0,
      right: 800,
      bottom: 300,
      width: 800,
      height: 300,
      toJSON: () => ({}),
    }) as DOMRect;
  URL.createObjectURL = vi.fn(() => "blob:signature");
  URL.revokeObjectURL = vi.fn();
  vi.stubGlobal("Image", FakeImage);
});

afterEach(() => {
  HTMLCanvasElement.prototype.getContext = originalGetContext;
  HTMLCanvasElement.prototype.toDataURL = originalToDataURL;
  HTMLCanvasElement.prototype.getBoundingClientRect = originalGetBoundingClientRect;
  URL.createObjectURL = originalCreateObjectUrl;
  URL.revokeObjectURL = originalRevokeObjectUrl;
  vi.unstubAllGlobals();
});

function renderDialog() {
  const onInserted = vi.fn();
  const onCancel = vi.fn();
  render(
    <SignatureDialog
      documentId="doc-1"
      pageIndex={2}
      onInserted={onInserted}
      onCancel={onCancel}
    />,
  );
  return { onInserted, onCancel };
}

function drawOnCanvas() {
  const canvas = screen.getByTestId("signature-canvas");
  fireEvent.pointerDown(canvas, { clientX: 100, clientY: 60, pointerId: 1 });
  fireEvent.pointerMove(canvas, { clientX: 160, clientY: 90, pointerId: 1 });
  fireEvent.pointerMove(canvas, { clientX: 220, clientY: 70, pointerId: 1 });
  fireEvent.pointerUp(canvas, { clientX: 220, clientY: 70, pointerId: 1 });
}

const pngFile = () =>
  new File(
    [new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])],
    "signature.png",
    { type: "image/png" },
  );

describe("SignatureDialog", () => {
  it("opens as a Chinese modal with the three entry points and focuses the first tab", () => {
    renderDialog();
    expect(screen.getByRole("dialog", { name: "插入签名" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "绘制" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByRole("tab", { name: "输入" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "上传" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "绘制" })).toHaveFocus();
    expect(screen.getByRole("button", { name: "插入签名" })).toBeDisabled();
  });

  it("draws strokes, supports undo and clear, and inserts a transparent PNG", async () => {
    const { onInserted, onCancel } = renderDialog();

    drawOnCanvas();
    expect(screen.getByRole("button", { name: "插入签名" })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "撤销" }));
    expect(screen.getByRole("button", { name: "插入签名" })).toBeDisabled();
    drawOnCanvas();
    fireEvent.click(screen.getByRole("button", { name: "清除" }));
    expect(screen.getByRole("button", { name: "插入签名" })).toBeDisabled();

    drawOnCanvas();
    fireEvent.click(screen.getByRole("button", { name: "插入签名" }));

    await waitFor(() => {
      expect(insertSignatureImageMock).toHaveBeenCalledWith(
        "doc-1",
        2,
        "QUJD",
        "手绘签名",
      );
    });
    expect(onInserted).toHaveBeenCalledWith(signatureImage);
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("shows a busy state and disables every action while inserting", async () => {
    let resolveInsert: (value: ImageEditResultView) => void = () => {};
    insertSignatureImageMock.mockReturnValue(
      new Promise<ImageEditResultView>((resolve) => {
        resolveInsert = resolve;
      }),
    );
    renderDialog();

    drawOnCanvas();
    fireEvent.click(screen.getByRole("button", { name: "插入签名" }));

    expect(await screen.findByRole("button", { name: "正在插入…" })).toBeDisabled();
    expect(screen.getByRole("tab", { name: "输入" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "取消" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(screen.queryByRole("dialog")).toBeInTheDocument();

    resolveInsert(inserted());
    await waitFor(() => {
      expect(insertSignatureImageMock).toHaveBeenCalledTimes(1);
    });
  });

  it("keeps the dialog open and explains a rejected payload in Chinese", async () => {
    insertSignatureImageMock.mockRejectedValue({
      code: "image_too_large",
      message: "too large",
    });
    const { onInserted } = renderDialog();

    drawOnCanvas();
    fireEvent.click(screen.getByRole("button", { name: "插入签名" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "图片过大：文件不能超过 25 MB，像素不能超过 2000 万。",
    );
    expect(onInserted).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "插入签名" })).toBeEnabled();
  });

  it("types a Chinese name, validates it and inserts a rendered signature", async () => {
    renderDialog();
    fireEvent.click(screen.getByRole("tab", { name: "输入" }));

    expect(screen.getByText("请输入要签写的姓名。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "插入签名" })).toBeDisabled();

    fireEvent.change(screen.getByLabelText("姓名"), {
      target: { value: "名".repeat(SIGNATURE_MAX_NAME_CHARS + 1) },
    });
    expect(
      screen.getByText(`姓名不能超过 ${SIGNATURE_MAX_NAME_CHARS} 个字符。`),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "插入签名" })).toBeDisabled();

    fireEvent.change(screen.getByLabelText("姓名"), {
      target: { value: "张三" },
    });
    const preview = await screen.findByAltText("签名预览");
    expect(preview).toHaveAttribute("src", "data:image/png;base64,QUJD");
    fireEvent.click(screen.getByRole("button", { name: "插入签名" }));

    await waitFor(() => {
      expect(insertSignatureImageMock).toHaveBeenCalledWith(
        "doc-1",
        2,
        "QUJD",
        "输入签名",
      );
    });
  });

  it("uploads a PNG, preserves its alpha and offers background removal only for opaque files", async () => {
    renderDialog();
    fireEvent.click(screen.getByRole("tab", { name: "上传" }));
    expect(screen.getByText("选择一张 PNG 或 JPG 图片作为签名。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "插入签名" })).toBeDisabled();

    fireEvent.change(screen.getByLabelText("选择签名图片"), {
      target: { files: [pngFile()] },
    });

    const preview = await screen.findByAltText("签名预览");
    expect(preview).toHaveAttribute("src", "data:image/png;base64,QUJD");
    expect(screen.getByText("signature.png")).toBeInTheDocument();
    // The fake pixels are opaque white with one dark pixel, so the dialog
    // defaults to removing the background and says so.
    expect(screen.getByRole("checkbox", { name: "移除白色背景" })).toBeChecked();
    expect(
      screen.getByText("已移除白色背景并裁掉透明边距。"),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("checkbox", { name: "移除白色背景" }));
    expect(
      await screen.findByText(
        "图片背景不透明，签名将保留原始背景。可勾选“移除白色背景”。",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("checkbox", { name: "移除白色背景" }),
    ).not.toBeChecked();

    fireEvent.click(screen.getByRole("button", { name: "插入签名" }));
    await waitFor(() => {
      expect(insertSignatureImageMock).toHaveBeenCalledWith(
        "doc-1",
        2,
        "QUJD",
        "上传签名",
      );
    });
  });

  it("rejects a non-image upload with Chinese copy and no preview", async () => {
    renderDialog();
    fireEvent.click(screen.getByRole("tab", { name: "上传" }));

    fireEvent.change(screen.getByLabelText("选择签名图片"), {
      target: { files: [new File(["not an image"], "notes.txt", { type: "text/plain" })] },
    });

    expect(
      await screen.findByText("仅支持 PNG 和 JPG 格式的图片。"),
    ).toBeInTheDocument();
    expect(screen.queryByAltText("签名预览")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "插入签名" })).toBeDisabled();
  });

  it("cancels with the button or Escape and keeps the document untouched", () => {
    const { onCancel, onInserted } = renderDialog();
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(onCancel).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onCancel).toHaveBeenCalledTimes(2);
    expect(insertSignatureImageMock).not.toHaveBeenCalled();
    expect(onInserted).not.toHaveBeenCalled();
  });
});
