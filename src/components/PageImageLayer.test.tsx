import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  EditHistoryView,
  ImageEditResultView,
  ImageObjectView,
} from "../lib/backend";
import { chooseImageFile, createLink, deleteImage, insertImage, pageImages, transformImage } from "../lib/backend";
import { DOCUMENT_CHANGED_EVENT } from "../lib/documentEvents";
import { PageImageLayer } from "./PageImageLayer";

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

const image: ImageObjectView = {
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

const inserted: ImageEditResultView = {
  pageIndex: 0,
  image,
  history: idleHistory,
};

function transformed(next: Partial<ImageObjectView>): ImageEditResultView {
  return { pageIndex: 0, image: { ...image, ...next }, history: idleHistory };
}

const originalGetBoundingClientRect = Element.prototype.getBoundingClientRect;

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
  Element.prototype.getBoundingClientRect = originalGetBoundingClientRect;
});

function renderLayer(
  readOnly = false,
  props: Partial<Parameters<typeof PageImageLayer>[0]> = {},
) {
  return render(
    <PageImageLayer
      documentId="doc-1"
      pageIndex={0}
      pageWidth={612}
      pageHeight={792}
      readOnly={readOnly}
      pageCount={10}
      {...props}
    />,
  );
}

describe("PageImageLayer", () => {
  it("inserts a chosen image and positions its overlay relative to the page", async () => {
    chooseImageFileMock.mockResolvedValue("/tmp/photo.png");
    insertImageMock.mockResolvedValue(inserted);
    // The layer refreshes its canonical list after the document-change event.
    pageImagesMock.mockResolvedValue([image]);
    const changed = vi.fn();
    window.addEventListener(DOCUMENT_CHANGED_EVENT, changed);

    renderLayer();
    fireEvent.click(await screen.findByRole("button", { name: "插入图片…" }));

    await waitFor(() => {
      expect(insertImageMock).toHaveBeenCalledWith("doc-1", 0, "/tmp/photo.png");
    });
    const outline = await screen.findByTestId("image-outline-1");
    expect(outline).toHaveStyle({
      left: `${(200 / 612) * 100}%`,
      top: `${(300 / 792) * 100}%`,
      width: `${(200 / 612) * 100}%`,
      height: `${(100 / 792) * 100}%`,
    });
    expect(outline).toHaveTextContent("photo.png");
    expect(changed).toHaveBeenCalled();
    window.removeEventListener(DOCUMENT_CHANGED_EVENT, changed);
  });

  it("does not open the dialog when the document is read-only", async () => {
    renderLayer(true);

    const button = await screen.findByRole("button", { name: "插入图片…" });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute("title", "此文档不允许编辑内容");
    expect(pageImagesMock).toHaveBeenCalledWith("doc-1", 0);
    fireEvent.click(button);
    expect(chooseImageFileMock).not.toHaveBeenCalled();
  });

  it("shows a Chinese error and no overlay when the file is rejected", async () => {
    chooseImageFileMock.mockResolvedValue("/tmp/notes.txt");
    insertImageMock.mockRejectedValue({
      code: "unsupported_image_format",
      message: "unsupported image extension",
    });

    renderLayer();
    fireEvent.click(await screen.findByRole("button", { name: "插入图片…" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "仅支持 PNG 和 JPG 格式的图片。",
    );
    expect(screen.queryByTestId("image-outline-1")).not.toBeInTheDocument();
  });

  it("deletes the selected image and lets the page refresh", async () => {
    pageImagesMock.mockResolvedValue([image]);
    deleteImageMock.mockResolvedValue({
      pageIndex: 0,
      pageCount: 1,
      pageStructureChanged: false,
      history: idleHistory,
    });
    const changed = vi.fn();
    window.addEventListener(DOCUMENT_CHANGED_EVENT, changed);

    renderLayer();
    const outline = await screen.findByTestId("image-outline-1");
    fireEvent.pointerDown(outline, { clientX: 300, clientY: 350, pointerId: 1 });
    fireEvent.pointerUp(outline, { clientX: 300, clientY: 350, pointerId: 1 });
    fireEvent.click(screen.getByRole("button", { name: "删除图片" }));

    await waitFor(() => {
      expect(deleteImageMock).toHaveBeenCalledWith("doc-1", 0, 1);
    });
    await waitFor(() => {
      expect(screen.queryByTestId("image-outline-1")).not.toBeInTheDocument();
    });
    expect(changed).toHaveBeenCalledWith(
      expect.objectContaining({
        detail: expect.objectContaining({
          documentId: "doc-1",
          pageIndex: 0,
          source: "image",
        }),
      }),
    );
    window.removeEventListener(DOCUMENT_CHANGED_EVENT, changed);
  });

  it("moves an image and commits the new position as one action", async () => {
    pageImagesMock.mockResolvedValue([image]);
    transformImageMock.mockResolvedValue(transformed({ x: 250, y: 350 }));

    renderLayer();
    const outline = await screen.findByTestId("image-outline-1");
    fireEvent.pointerDown(outline, { clientX: 300, clientY: 350, pointerId: 1 });
    fireEvent.pointerMove(outline, { clientX: 350, clientY: 400, pointerId: 1 });
    fireEvent.pointerUp(outline, { clientX: 350, clientY: 400, pointerId: 1 });

    await waitFor(() => {
      expect(transformImageMock).toHaveBeenCalledWith("doc-1", 0, 1, {
        x: 250,
        y: 350,
        width: 200,
        height: 100,
        rotation: 0,
      });
    });
    await waitFor(() => {
      expect(screen.getByTestId("image-outline-1")).toHaveStyle({
        left: `${(250 / 612) * 100}%`,
        top: `${(350 / 792) * 100}%`,
      });
    });
  });

  it("resizes an image around its center while preserving the aspect ratio", async () => {
    pageImagesMock.mockResolvedValue([image]);
    // Center (300, 350). The bottom-right handle is at (400, 400); dragging
    // half-way along the diagonal gives a 100x50 image centered in place.
    transformImageMock.mockResolvedValue(
      transformed({ x: 250, y: 325, width: 100, height: 50 }),
    );

    renderLayer();
    const outline = await screen.findByTestId("image-outline-1");
    fireEvent.pointerDown(outline, { clientX: 300, clientY: 350, pointerId: 1 });
    fireEvent.pointerUp(outline, { clientX: 300, clientY: 350, pointerId: 1 });
    const handle = await screen.findByRole("button", { name: "调整图片大小" });
    fireEvent.pointerDown(handle, { clientX: 400, clientY: 400, pointerId: 2 });
    fireEvent.pointerMove(handle, { clientX: 350, clientY: 375, pointerId: 2 });
    fireEvent.pointerUp(handle, { clientX: 350, clientY: 375, pointerId: 2 });

    await waitFor(() => {
      expect(transformImageMock).toHaveBeenCalledWith("doc-1", 0, 1, {
        x: 250,
        y: 325,
        width: 100,
        height: 50,
        rotation: 0,
      });
    });
  });

  it("rotates an image around its center", async () => {
    pageImagesMock.mockResolvedValue([image]);
    transformImageMock.mockResolvedValue(transformed({ rotation: 90 }));

    renderLayer();
    const outline = await screen.findByTestId("image-outline-1");
    fireEvent.pointerDown(outline, { clientX: 300, clientY: 350, pointerId: 1 });
    fireEvent.pointerUp(outline, { clientX: 300, clientY: 350, pointerId: 1 });
    const handle = await screen.findByRole("button", { name: "旋转图片" });
    fireEvent.pointerDown(handle, { clientX: 300, clientY: 300, pointerId: 2 });
    fireEvent.pointerMove(handle, { clientX: 400, clientY: 350, pointerId: 2 });
    fireEvent.pointerUp(handle, { clientX: 400, clientY: 350, pointerId: 2 });

    await waitFor(() => {
      expect(transformImageMock).toHaveBeenCalledWith("doc-1", 0, 1, {
        x: 200,
        y: 300,
        width: 200,
        height: 100,
        rotation: 90,
      });
    });
    await waitFor(() => {
      expect(screen.getByTestId("image-outline-1").style.transform).toContain(
        "rotate(90deg)",
      );
    });
  });

  it("warns that the page clips an image that extends past it", async () => {
    pageImagesMock.mockResolvedValue([{ ...image, x: -50 }]);

    renderLayer();
    const outline = await screen.findByTestId("image-outline-1");
    fireEvent.pointerDown(outline, { clientX: 50, clientY: 350, pointerId: 1 });
    fireEvent.pointerUp(outline, { clientX: 50, clientY: 350, pointerId: 1 });

    expect(
      await screen.findByText("图片超出页面，超出部分将被裁切。"),
    ).toBeInTheDocument();
  });

  it("creates a link over the selected image's bounds", async () => {
    pageImagesMock.mockResolvedValue([image]);
    createLinkMock.mockResolvedValue({
      link: {
        id: "0:0",
        name: "pdfx-link-1",
        pageIndex: 0,
        target: { kind: "url", url: "https://example.com/photo" },
        unsupported: null,
        rects: [{ x: 200, y: 300, width: 200, height: 100 }],
      },
      pageIndex: 0,
      history: idleHistory,
    });

    renderLayer(false, { canAnnotate: true });
    const outline = await screen.findByTestId("image-outline-1");
    fireEvent.pointerDown(outline, { clientX: 300, clientY: 350, pointerId: 1 });
    fireEvent.pointerUp(outline, { clientX: 300, clientY: 350, pointerId: 1 });

    fireEvent.click(screen.getByRole("button", { name: "添加链接" }));
    expect(screen.getByRole("dialog", { name: "添加链接" })).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("链接网址"), {
      target: { value: "https://example.com/photo" },
    });
    fireEvent.click(screen.getByRole("button", { name: "添加" }));

    await waitFor(() => {
      expect(createLinkMock).toHaveBeenCalledWith("doc-1", {
        pageIndex: 0,
        rects: [{ x: 200, y: 300, width: 200, height: 100 }],
        target: { kind: "url", url: "https://example.com/photo" },
      });
    });
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(screen.getByRole("status")).toHaveTextContent("链接已添加");
  });

  it("uses the axis-aligned bounds of a rotated image for the link", async () => {
    pageImagesMock.mockResolvedValue([{ ...image, rotation: 90 }]);
    createLinkMock.mockResolvedValue({
      link: null,
      pageIndex: 0,
      history: idleHistory,
    });

    renderLayer(false, { canAnnotate: true });
    const outline = await screen.findByTestId("image-outline-1");
    fireEvent.pointerDown(outline, { clientX: 300, clientY: 350, pointerId: 1 });
    fireEvent.pointerUp(outline, { clientX: 300, clientY: 350, pointerId: 1 });

    fireEvent.click(screen.getByRole("button", { name: "添加链接" }));
    fireEvent.click(screen.getByRole("radio", { name: "页面" }));
    fireEvent.change(screen.getByLabelText("目标页码"), {
      target: { value: "4" },
    });
    fireEvent.click(screen.getByRole("button", { name: "添加" }));

    await waitFor(() => {
      expect(createLinkMock).toHaveBeenCalledWith("doc-1", {
        pageIndex: 0,
        rects: [{ x: 250, y: 250, width: 100, height: 200 }],
        target: { kind: "page", pageIndex: 3 },
      });
    });
  });

  it("explains why an image cannot get a link without annotation permission", async () => {
    pageImagesMock.mockResolvedValue([image]);

    renderLayer(false, { canAnnotate: false });
    const outline = await screen.findByTestId("image-outline-1");
    fireEvent.pointerDown(outline, { clientX: 300, clientY: 350, pointerId: 1 });
    fireEvent.pointerUp(outline, { clientX: 300, clientY: 350, pointerId: 1 });

    const button = screen.getByRole("button", { name: "添加链接" });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute("title", "此文档不允许添加注释");
    expect(screen.getByText("此文档不允许添加注释")).toBeInTheDocument();
  });

  it("disables the image link action until an image is selected", async () => {
    pageImagesMock.mockResolvedValue([image]);

    renderLayer(false, { canAnnotate: true });

    expect(screen.getByRole("button", { name: "添加链接" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "添加链接" })).toHaveAttribute(
      "title",
      "先选择一张图片",
    );
  });
});
