import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  EditHistoryView,
  ImageObjectView,
} from "../lib/backend";
import {
  chooseImageFile,
  createLink,
  deleteImage,
  insertImage,
  pageImages,
  transformImage,
} from "../lib/backend";
import { DOCUMENT_CHANGED_EVENT } from "../lib/documentEvents";
import { PageImageLayer } from "./PageImageLayer";

vi.mock("./SignatureDialog", () => ({
  SignatureDialog: ({
    onInserted,
    onCancel,
  }: {
    onInserted: (image: ImageObjectView) => void;
    onCancel: () => void;
  }) => (
    <div role="dialog" aria-label="插入签名">
      <button
        type="button"
        onClick={() =>
          onInserted({
            imageId: 9,
            pageIndex: 0,
            name: "手绘签名",
            kind: "signature",
            x: 150,
            y: 300,
            width: 200,
            height: 75,
            rotation: 0,
            pixelWidth: 1600,
            pixelHeight: 600,
          })
        }
      >
        确认测试签名
      </button>
      <button type="button" onClick={onCancel}>
        取消测试
      </button>
    </div>
  ),
}));

vi.mock("../lib/backend", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/backend")>();
  return {
    ...actual,
    chooseImageFile: vi.fn(),
    createLink: vi.fn(),
    insertImage: vi.fn(),
    pageImages: vi.fn(),
    transformImage: vi.fn(),
    deleteImage: vi.fn(),
  };
});

const chooseImageFileMock = vi.mocked(chooseImageFile);
const createLinkMock = vi.mocked(createLink);
const insertImageMock = vi.mocked(insertImage);
const pageImagesMock = vi.mocked(pageImages);
const transformImageMock = vi.mocked(transformImage);
const deleteImageMock = vi.mocked(deleteImage);

const idleHistory: EditHistoryView = {
  canUndo: false,
  canRedo: false,
  revision: 0,
  savedRevision: 0,
  dirty: false,
};

const photo: ImageObjectView = {
  imageId: 1,
  pageIndex: 0,
  name: "photo.png",
  kind: "image",
  x: 200,
  y: 300,
  width: 200,
  height: 100,
  rotation: 0,
  pixelWidth: 400,
  pixelHeight: 200,
};

const signature: ImageObjectView = {
  imageId: 2,
  pageIndex: 0,
  name: "输入签名",
  kind: "signature",
  x: 150,
  y: 380,
  width: 220,
  height: 82,
  rotation: 0,
  pixelWidth: 1200,
  pixelHeight: 450,
};

beforeEach(() => {
  for (const mock of [
    chooseImageFileMock,
    createLinkMock,
    insertImageMock,
    pageImagesMock,
    transformImageMock,
    deleteImageMock,
  ]) {
    mock.mockReset();
  }
  pageImagesMock.mockResolvedValue([]);
  deleteImageMock.mockResolvedValue({
    pageIndex: 0,
    pageCount: 1,
    pageStructureChanged: false,
    history: idleHistory,
  });
});

function renderLayer(readOnly = false) {
  return render(
    <PageImageLayer
      documentId="doc-1"
      pageIndex={0}
      pageWidth={612}
      pageHeight={792}
      readOnly={readOnly}
      pageCount={1}
    />,
  );
}

describe("PageImageLayer signatures", () => {
  it("opens the signature dialog and adopts the inserted page object", async () => {
    const changed = vi.fn();
    window.addEventListener(DOCUMENT_CHANGED_EVENT, changed);
    renderLayer();

    const button = await screen.findByRole("button", { name: "插入签名…" });
    expect(button).toHaveAttribute(
      "title",
      "绘制、输入或上传签名并插入当前页",
    );
    fireEvent.click(button);
    expect(screen.getByRole("dialog", { name: "插入签名" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "确认测试签名" }));

    const outline = await screen.findByTestId("image-outline-9");
    expect(outline).toHaveAttribute("data-kind", "signature");
    expect(outline).toHaveAttribute("aria-label", "签名 手绘签名");
    expect(outline).toHaveTextContent("签名");
    expect(outline).toHaveTextContent("手绘签名");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(changed).toHaveBeenCalledWith(
      expect.objectContaining({
        detail: expect.objectContaining({
          documentId: "doc-1",
          pageIndex: 0,
          source: "image",
        }),
      }),
    );

    // The signature is selectable and deletable like any other image.
    fireEvent.pointerDown(outline, { clientX: 300, clientY: 350, pointerId: 1 });
    fireEvent.pointerUp(outline, { clientX: 300, clientY: 350, pointerId: 1 });
    fireEvent.click(screen.getByRole("button", { name: "删除图片" }));
    await waitFor(() => {
      expect(deleteImageMock).toHaveBeenCalledWith("doc-1", 0, 9);
    });
    await waitFor(() => {
      expect(screen.queryByTestId("image-outline-9")).not.toBeInTheDocument();
    });
    window.removeEventListener(DOCUMENT_CHANGED_EVENT, changed);
  });

  it("closes the dialog when the signature dialog is cancelled", async () => {
    renderLayer();
    fireEvent.click(await screen.findByRole("button", { name: "插入签名…" }));
    fireEvent.click(screen.getByRole("button", { name: "取消测试" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(pageImagesMock).toHaveBeenCalledTimes(1);
  });

  it("does not open the signature dialog for a read-only document", async () => {
    renderLayer(true);
    const button = await screen.findByRole("button", { name: "插入签名…" });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute("title", "此文档不允许编辑内容");
    fireEvent.click(button);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("keeps photos labelled as photos while showing signatures as signatures", async () => {
    pageImagesMock.mockResolvedValue([photo, signature]);
    renderLayer();

    const photoOutline = await screen.findByTestId("image-outline-1");
    expect(photoOutline).toHaveAttribute("data-kind", "image");
    expect(photoOutline).toHaveAttribute("aria-label", "图片 photo.png");
    expect(photoOutline).not.toHaveTextContent("签名");

    const signatureOutline = screen.getByTestId("image-outline-2");
    expect(signatureOutline).toHaveAttribute("aria-label", "签名 输入签名");
    expect(signatureOutline).toHaveTextContent("签名");
    expect(signatureOutline).toHaveTextContent("输入签名");
  });
});
