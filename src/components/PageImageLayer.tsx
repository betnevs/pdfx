import {
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { createPortal } from "react-dom";

import {
  chooseImageFile,
  createLink,
  deleteImage,
  insertImage,
  pageImages,
  toCommandError,
  transformImage,
  type CommandErrorCode,
  type ImageObjectView,
  type LinkTarget,
  type TextMarkupRect,
} from "../lib/backend";
import { notifyDocumentChanged, onDocumentChanged } from "../lib/documentEvents";
import { linkErrorText } from "../lib/linkTarget";
import {
  imageCorners,
  imageRotationFromPointer,
  isImageOutsidePage,
  moveImage,
  resizeImage,
  type ImageRect,
  type Point,
} from "../lib/imageGeometry";
import { LinkTargetDialog } from "./LinkTargetDialog";

const READ_ONLY_REASON = "此文档不允许编辑内容";
const ANNOTATION_PERMISSION_REASON = "此文档不允许添加注释";
const OUTSIDE_PAGE_WARNING = "图片超出页面，超出部分将被裁切。";
const NO_LINK_AREA_REASON = "图片不在页面可见区域内，无法添加链接。";

/** Chinese copy for failures specific to image files and placement. */
const IMAGE_ERROR_MESSAGES: Partial<Record<CommandErrorCode, string>> = {
  unsupported_image_format: "仅支持 PNG 和 JPG 格式的图片。",
  image_decode_failed: "无法读取这张图片，请确认文件是有效的 PNG 或 JPG。",
  image_too_large: "图片过大：文件不能超过 25 MB，像素不能超过 2000 万。",
  file_not_found: "找不到该图片文件。",
  file_read_failed: "无法读取该图片文件。",
  permission_denied: "文档所有者限制了编辑权限。",
  document_not_found: "文档已关闭，请重新打开。",
  page_out_of_range: "页码无效。",
  invalid_argument: "图片位置或大小无效。",
};

function imageErrorText(error: unknown): string {
  const commandError = toCommandError(error);
  return IMAGE_ERROR_MESSAGES[commandError.code] ?? "图片操作失败，请重试。";
}

interface Notice {
  kind: "info" | "error";
  text: string;
}

type Interaction =
  | {
      kind: "move";
      image: ImageObjectView;
      start: Point;
      current: Point;
    }
  | {
      kind: "resize";
      image: ImageObjectView;
      current: Point;
    }
  | {
      kind: "rotate";
      image: ImageObjectView;
      current: Point;
      snapDegrees: number;
    };

export interface PageImageLayerProps {
  documentId: string;
  pageIndex: number;
  /** Display-space page width, including CropBox and rotation. */
  pageWidth: number;
  /** Display-space page height, including CropBox and rotation. */
  pageHeight: number;
  /** Permission Restrictions forbid changing document content. */
  readOnly?: boolean;
  /** The PDF security handler allows adding annotation links. */
  canAnnotate?: boolean;
  /** Number of pages an internal link may address. */
  pageCount?: number;
}

/** The image geometry a drag currently previews. */
function previewFor(
  image: ImageObjectView,
  interaction: Interaction | null,
): ImageObjectView {
  if (!interaction || interaction.image.imageId !== image.imageId) {
    return image;
  }
  if (interaction.kind === "move") {
    return {
      ...image,
      ...moveImage(image, {
        x: interaction.current.x - interaction.start.x,
        y: interaction.current.y - interaction.start.y,
      }),
    };
  }
  if (interaction.kind === "resize") {
    return {
      ...image,
      ...resizeImage(image, image.rotation, interaction.current),
    };
  }
  return {
    ...image,
    rotation: imageRotationFromPointer(
      image,
      interaction.current,
      interaction.snapDegrees,
    ),
  };
}

/**
 * Image overlay for one Page.
 *
 * Images are positioned as percentages of the page box, so the layer stays
 * aligned at every zoom level. Moving, resizing (aspect-preserving, around
 * the center) and rotating (around the center) are previewed locally and
 * committed through one `transform_image` call on pointer release, which the
 * shared Edit Session records as a single undoable action.
 *
 * An image may extend beyond the page. That is deliberate: the page clips it
 * in the overlay and the saved PDF shows only the part inside the page box.
 * A warning states this whenever the selected image is partly outside.
 */
export function PageImageLayer({
  documentId,
  pageIndex,
  pageWidth,
  pageHeight,
  readOnly = false,
  canAnnotate = false,
  pageCount = 0,
}: PageImageLayerProps) {
  const [images, setImages] = useState<ImageObjectView[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [interaction, setInteraction] = useState<Interaction | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [linkDraft, setLinkDraft] = useState<{
    rects: TextMarkupRect[];
  } | null>(null);
  const [linkBusy, setLinkBusy] = useState(false);
  const [linkError, setLinkError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const layerRef = useRef<HTMLDivElement>(null);
  const pageKey = useRef<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const key = `${documentId}:${pageIndex}`;
    if (pageKey.current !== key) {
      pageKey.current = key;
      setImages([]);
      setSelectedId(null);
      setInteraction(null);
      setNotice(null);
    }
    pageImages(documentId, pageIndex)
      .then((list) => {
        if (!cancelled) {
          setImages(list);
          setSelectedId((current) =>
            current !== null && list.some((entry) => entry.imageId === current)
              ? current
              : null,
          );
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setImages([]);
          setSelectedId(null);
          setNotice({ kind: "error", text: imageErrorText(error) });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [documentId, pageIndex, reloadKey]);

  useEffect(
    () =>
      onDocumentChanged((detail) => {
        // This layer already updates its state from the command result of its
        // own actions, and its server-side list may lag behind that result;
        // refreshing on its own event would overwrite the committed preview.
        // Undo, redo and other tools notify with a different source.
        if (
          detail.documentId === documentId &&
          detail.pageIndex === pageIndex &&
          detail.source !== "image"
        ) {
          setReloadKey((value) => value + 1);
        }
      }),
    [documentId, pageIndex],
  );

  const selected =
    selectedId !== null
      ? (images.find((entry) => entry.imageId === selectedId) ?? null)
      : null;
  const preview = selected ? previewFor(selected, interaction) : null;
  const outsidePage =
    preview !== null &&
    isImageOutsidePage(preview, preview.rotation, pageWidth, pageHeight);

  function pointFromEvent(event: {
    clientX: number;
    clientY: number;
  }): Point | null {
    const layer = layerRef.current;
    if (!layer || pageWidth <= 0 || pageHeight <= 0) {
      return null;
    }
    const box = layer.getBoundingClientRect();
    if (box.width <= 0 || box.height <= 0) {
      return null;
    }
    return {
      x: ((event.clientX - box.left) / box.width) * pageWidth,
      y: ((event.clientY - box.top) / box.height) * pageHeight,
    };
  }

  async function chooseAndInsert() {
    if (readOnly || busy) {
      return;
    }
    let path: string | null;
    try {
      path = await chooseImageFile();
    } catch (error) {
      setNotice({ kind: "error", text: imageErrorText(error) });
      return;
    }
    if (!path) {
      return;
    }
    setBusy(true);
    try {
      const result = await insertImage(documentId, pageIndex, path);
      // The command result is canonical; replace an entry with the same id so
      // a stale list can never show the image twice.
      setImages((list) => [
        ...list.filter((entry) => entry.imageId !== result.image.imageId),
        result.image,
      ]);
      setSelectedId(result.image.imageId);
      setNotice(null);
      notifyDocumentChanged({ documentId, pageIndex, source: "image" });
    } catch (error) {
      setNotice({ kind: "error", text: imageErrorText(error) });
    } finally {
      setBusy(false);
    }
  }

  async function removeSelected() {
    if (!selected || readOnly || busy) {
      return;
    }
    setBusy(true);
    try {
      await deleteImage(documentId, pageIndex, selected.imageId);
      setImages((list) =>
        list.filter((entry) => entry.imageId !== selected.imageId),
      );
      setSelectedId(null);
      setInteraction(null);
      setNotice(null);
      notifyDocumentChanged({ documentId, pageIndex, source: "image" });
    } catch (error) {
      setNotice({ kind: "error", text: imageErrorText(error) });
    } finally {
      setBusy(false);
    }
  }

  /**
   * The axis-aligned display-space bounds of the image's rotated corners,
   * clipped to the visible page. A link must cover what the reader can click,
   * which is exactly the part of the image inside the page box.
   */
  function linkBoundsFor(image: ImageObjectView): TextMarkupRect | null {
    const corners = imageCorners(image, image.rotation);
    const left = Math.max(0, Math.min(...corners.map((corner) => corner.x)));
    const top = Math.max(0, Math.min(...corners.map((corner) => corner.y)));
    const right = Math.min(
      pageWidth,
      Math.max(...corners.map((corner) => corner.x)),
    );
    const bottom = Math.min(
      pageHeight,
      Math.max(...corners.map((corner) => corner.y)),
    );
    const width = right - left;
    const height = bottom - top;
    if (width <= 0.5 || height <= 0.5) {
      return null;
    }
    return { x: left, y: top, width, height };
  }

  function openImageLinkDialog() {
    if (readOnly || !canAnnotate || busy || linkBusy || !selected) {
      return;
    }
    const rect = linkBoundsFor(selected);
    if (!rect) {
      setNotice({ kind: "error", text: NO_LINK_AREA_REASON });
      return;
    }
    setLinkError(null);
    setLinkDraft({ rects: [rect] });
  }

  async function createLinkForImage(target: LinkTarget) {
    if (!linkDraft || linkBusy) {
      return;
    }
    setLinkBusy(true);
    setLinkError(null);
    try {
      await createLink(documentId, {
        pageIndex,
        rects: linkDraft.rects,
        target,
      });
      setLinkDraft(null);
      setNotice({ kind: "info", text: "链接已添加" });
      notifyDocumentChanged({ documentId, pageIndex, source: "link" });
    } catch (error) {
      setLinkError(linkErrorText(error));
    } finally {
      setLinkBusy(false);
    }
  }

  async function commitTransform(image: ImageObjectView, next: ImageRect & {
    rotation: number;
  }) {
    if (
      next.x === image.x &&
      next.y === image.y &&
      next.width === image.width &&
      next.height === image.height &&
      next.rotation === image.rotation
    ) {
      return;
    }
    setBusy(true);
    try {
      const result = await transformImage(documentId, pageIndex, image.imageId, {
        x: next.x,
        y: next.y,
        width: next.width,
        height: next.height,
        rotation: next.rotation,
      });
      setImages((list) =>
        list.map((entry) =>
          entry.imageId === result.image.imageId ? result.image : entry,
        ),
      );
      setNotice(null);
      notifyDocumentChanged({ documentId, pageIndex, source: "image" });
    } catch (error) {
      setNotice({ kind: "error", text: imageErrorText(error) });
    } finally {
      setBusy(false);
    }
  }

  const startMove = (
    event: ReactPointerEvent<HTMLDivElement>,
    image: ImageObjectView,
  ) => {    event.stopPropagation();
    setSelectedId(image.imageId);
    if (readOnly || busy) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    event.currentTarget.setPointerCapture?.(event.pointerId);
    setInteraction({ kind: "move", image, start: point, current: point });
  };

  const startResize = (
    event: ReactPointerEvent<HTMLButtonElement>,
    image: ImageObjectView,
  ) => {
    event.stopPropagation();
    if (readOnly || busy) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    event.currentTarget.setPointerCapture?.(event.pointerId);
    setInteraction({ kind: "resize", image, current: point });
  };

  const startRotate = (
    event: ReactPointerEvent<HTMLButtonElement>,
    image: ImageObjectView,
  ) => {
    event.stopPropagation();
    if (readOnly || busy) {
      return;
    }
    const point = pointFromEvent(event);
    if (!point) {
      return;
    }
    event.currentTarget.setPointerCapture?.(event.pointerId);
    setInteraction({
      kind: "rotate",
      image,
      current: point,
      snapDegrees: event.shiftKey ? 15 : 0,
    });
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!interaction) {
      return;
    }
    const point = pointFromEvent(event) ?? interaction.current;
    setInteraction(
      interaction.kind === "rotate"
        ? { ...interaction, current: point, snapDegrees: event.shiftKey ? 15 : 0 }
        : { ...interaction, current: point },
    );
  };

  const finishInteraction = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!interaction) {
      return;
    }
    const point = pointFromEvent(event) ?? interaction.current;
    const image = interaction.image;
    const settled =
      interaction.kind === "rotate"
        ? { ...interaction, current: point, snapDegrees: event.shiftKey ? 15 : 0 }
        : { ...interaction, current: point };
    const next = previewFor(image, settled);
    setInteraction(null);
    try {
      event.currentTarget.releasePointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture may never have been acquired.
    }
    void commitTransform(image, {
      x: next.x,
      y: next.y,
      width: next.width,
      height: next.height,
      rotation: next.rotation,
    });
  };

  return (
    <div
      ref={layerRef}
      className="page-image-layer"
      role="group"
      aria-label="图片层"
      data-testid="page-image-layer"
      onPointerMove={handlePointerMove}
      onPointerUp={finishInteraction}
      onPointerCancel={() => setInteraction(null)}
    >
      <div
        className="image-edit-toolbar"
        onPointerDown={(event) => event.stopPropagation()}
      >
        <button
          type="button"
          disabled={readOnly || busy}
          title={readOnly ? READ_ONLY_REASON : "选择一张 PNG 或 JPG 图片插入当前页"}
          onClick={() => void chooseAndInsert()}
        >
          插入图片…
        </button>
        <button
          type="button"
          disabled={readOnly || busy || selected === null}
          title={
            readOnly
              ? READ_ONLY_REASON
              : selected === null
                ? "先选择一张图片"
                : "删除选中的图片（可撤销）"
          }
          onClick={() => void removeSelected()}
        >
          删除图片
        </button>
        <button
          type="button"
          disabled={readOnly || !canAnnotate || busy || linkBusy || selected === null}
          title={
            readOnly
              ? READ_ONLY_REASON
              : !canAnnotate
                ? ANNOTATION_PERMISSION_REASON
                : selected === null
                  ? "先选择一张图片"
                  : "为选中的图片添加超链接"
          }
          onClick={openImageLinkDialog}
        >
          添加链接
        </button>
        {readOnly && <span className="image-edit-reason">{READ_ONLY_REASON}</span>}
        {!readOnly && !canAnnotate && (
          <span className="image-edit-reason">
            {ANNOTATION_PERMISSION_REASON}
          </span>
        )}
        {outsidePage && (
          <span className="image-edit-warning" role="status">
            {OUTSIDE_PAGE_WARNING}
          </span>
        )}
        {notice && (
          <span
            className={`image-edit-notice image-edit-notice--${notice.kind}`}
            role={notice.kind === "error" ? "alert" : "status"}
          >
            {notice.text}
          </span>
        )}
      </div>

      {images.map((image) => {
        const rect = previewFor(image, interaction);
        const isSelected = selectedId === image.imageId;
        return (
          <div
            key={image.imageId}
            className={`image-outline${isSelected ? " image-outline--selected" : ""}`}
            role="group"
            aria-label={`图片 ${image.name}`}
            data-testid={`image-outline-${image.imageId}`}
            title={
              isSelected && outsidePage
                ? OUTSIDE_PAGE_WARNING
                : `${image.name}（可拖动、调整大小或旋转）`
            }
            style={{
              left: `${(rect.x / pageWidth) * 100}%`,
              top: `${(rect.y / pageHeight) * 100}%`,
              width: `${(rect.width / pageWidth) * 100}%`,
              height: `${(rect.height / pageHeight) * 100}%`,
              transform: `rotate(${rect.rotation}deg)`,
            }}
            onPointerDown={(event) => startMove(event, image)}
          >
            <span className="image-outline-label">{image.name}</span>
            {isSelected && !readOnly && (
              <>
                <button
                  type="button"
                  className="image-handle image-handle--rotate"
                  aria-label="旋转图片"
                  title="拖动以旋转（按住 Shift 每次 15 度）"
                  onPointerDown={(event) => startRotate(event, image)}
                />
                <button
                  type="button"
                  className="image-handle image-handle--resize"
                  aria-label="调整图片大小"
                  title="拖动以调整大小，保持宽高比"
                  onPointerDown={(event) => startResize(event, image)}
                />
              </>
            )}
          </div>
        );
      })}

      {linkDraft &&
        createPortal(
          <LinkTargetDialog
            mode="create"
            pageCount={pageCount}
            initialTarget={null}
            busy={linkBusy}
            error={linkError}
            onCancel={() => {
              if (!linkBusy) {
                setLinkDraft(null);
                setLinkError(null);
              }
            }}
            onSubmit={(target) => void createLinkForImage(target)}
          />,
          document.body,
        )}
    </div>
  );
}
