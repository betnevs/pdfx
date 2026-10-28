import {
  useEffect,
  useLayoutEffect,
  useReducer,
  useRef,
  useState,
  type DragEvent as ReactDragEvent,
  type RefObject,
} from "react";

import {
  renderThumbnail,
  type AnnotationView,
  type LinkView,
  type PageImageView,
  type PageInfo,
  type PageRegionView,
  type StampKind,
} from "../lib/backend";
import { onDocumentChanged } from "../lib/documentEvents";
import {
  pagePixelSize,
  percentToScale,
  scaleToPercent,
  THUMBNAIL_MAX_EDGE,
  type FitMode,
} from "../lib/pageGeometry";
import type { PageRender, ZoomMode } from "../lib/useDocumentViewer";
import { EraserLayer } from "./EraserLayer";
import { FormLayer } from "./FormLayer";
import { FreeTextLayer } from "./FreeTextLayer";
import { LineLayer } from "./LineLayer";
import { LinkLayer } from "./LinkLayer";
import { PageImageLayer } from "./PageImageLayer";
import { PageTextLayer } from "./PageTextLayer";
import { ShapeLayer } from "./ShapeLayer";
import { StampLayer } from "./StampLayer";

/** Fixed cell height keeps thumbnail virtualization independent of page sizes. */
const THUMBNAIL_ITEM_HEIGHT = 168;
const THUMBNAIL_OVERSCAN = 2;
/** Used until the rail has been measured (zero height in tests and first paint). */
const THUMBNAIL_FALLBACK_VIEWPORT = 600;

interface ThumbnailRange {
  start: number;
  end: number;
}

function thumbnailRange(
  scrollTop: number,
  viewportHeight: number,
  pageCount: number,
): ThumbnailRange {
  const first = Math.floor(scrollTop / THUMBNAIL_ITEM_HEIGHT);
  const visible = Math.ceil(viewportHeight / THUMBNAIL_ITEM_HEIGHT);
  return {
    start: Math.max(0, first - THUMBNAIL_OVERSCAN),
    end: Math.min(pageCount, first + visible + THUMBNAIL_OVERSCAN),
  };
}

export function DocumentToolbar({
  pageIndex,
  pageCount,
  scale,
  zoomMode,
  fullscreen,
  onSelectPage,
  onZoomTo,
  onZoomIn,
  onZoomOut,
  onFit,
  onActualSize,
  onToggleFullscreen,
  canModifyPages = false,
  pageOperationDisabledReason = null,
  onRotatePage,
  onCopyPage,
  onDeletePage,
  onInsertPage,
}: {
  pageIndex: number;
  pageCount: number;
  scale: number;
  zoomMode: ZoomMode;
  fullscreen: boolean;
  onSelectPage: (index: number) => void;
  onZoomTo: (scale: number) => void;
  onZoomIn: () => void;
  onZoomOut: () => void;
  onFit: (mode: FitMode) => void;
  onActualSize: () => void;
  onToggleFullscreen: () => void;
  /** Permission Restrictions allow changing the page arrangement. */
  canModifyPages?: boolean;
  /** Chinese explanation shown while page operations are unavailable. */
  pageOperationDisabledReason?: string | null;
  onRotatePage?: (deltaDegrees: 90 | 180 | 270) => void;
  onCopyPage?: () => void;
  onDeletePage?: () => void;
  onInsertPage?: () => void;
}) {
  const [pageDraft, setPageDraft] = useState(String(pageIndex + 1));
  const [zoomDraft, setZoomDraft] = useState(`${scaleToPercent(scale)}%`);

  useEffect(() => {
    setPageDraft(String(pageIndex + 1));
  }, [pageIndex]);

  useEffect(() => {
    setZoomDraft(`${scaleToPercent(scale)}%`);
  }, [scale]);

  const commitPage = () => {
    const parsed = Number.parseInt(pageDraft, 10);
    if (!Number.isFinite(parsed)) {
      setPageDraft(String(pageIndex + 1));
      return;
    }
    const next = Math.min(pageCount, Math.max(1, parsed));
    setPageDraft(String(next));
    onSelectPage(next - 1);
  };

  const commitZoom = () => {
    const parsed = Number.parseFloat(zoomDraft.replace("%", "").trim());
    if (!Number.isFinite(parsed)) {
      setZoomDraft(`${scaleToPercent(scale)}%`);
      return;
    }
    const next = percentToScale(parsed);
    setZoomDraft(`${scaleToPercent(next)}%`);
    onZoomTo(next);
  };

  return (
    <div className="document-toolbar">
      <div className="page-nav" role="group" aria-label="翻页">
        <button
          type="button"
          aria-label="上一页"
          disabled={pageIndex <= 0}
          onClick={() => onSelectPage(pageIndex - 1)}
        >
          上一页
        </button>
        <input
          type="number"
          className="page-input"
          aria-label="当前页码"
          min={1}
          max={pageCount}
          value={pageDraft}
          onChange={(event) => setPageDraft(event.target.value)}
          onBlur={commitPage}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commitPage();
            }
          }}
        />
        <span className="page-total">/ {pageCount}</span>
        <button
          type="button"
          aria-label="下一页"
          disabled={pageIndex >= pageCount - 1}
          onClick={() => onSelectPage(pageIndex + 1)}
        >
          下一页
        </button>
      </div>

      <div
        className="page-ops"
        role="group"
        aria-label="页面操作"
        title={pageOperationDisabledReason ?? undefined}
      >
        <button
          type="button"
          title="逆时针旋转 90°"
          disabled={!canModifyPages}
          onClick={() => onRotatePage?.(270)}
        >
          向左旋转
        </button>
        <button
          type="button"
          title="顺时针旋转 90°"
          disabled={!canModifyPages}
          onClick={() => onRotatePage?.(90)}
        >
          向右旋转
        </button>
        <button
          type="button"
          disabled={!canModifyPages}
          onClick={() => onInsertPage?.()}
        >
          插入空白页…
        </button>
        <button
          type="button"
          title="复制当前页并插入到其后（可撤销）"
          disabled={!canModifyPages}
          onClick={() => onCopyPage?.()}
        >
          复制页面
        </button>
        <button
          type="button"
          disabled={!canModifyPages || pageCount <= 1}
          title={
            pageCount <= 1 ? "文档至少需要保留一页" : "删除当前页（可撤销）"
          }
          onClick={() => onDeletePage?.()}
        >
          删除页面
        </button>
        {pageOperationDisabledReason && (
          <span className="page-ops-hint">{pageOperationDisabledReason}</span>
        )}
      </div>

      <div className="zoom-controls" role="group" aria-label="缩放">
        <button type="button" aria-label="缩小" onClick={onZoomOut}>
          缩小
        </button>
        <input
          type="text"
          className="zoom-input"
          aria-label="缩放比例"
          inputMode="decimal"
          value={zoomDraft}
          onChange={(event) => setZoomDraft(event.target.value)}
          onBlur={commitZoom}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commitZoom();
            }
          }}
        />
        <button type="button" aria-label="放大" onClick={onZoomIn}>
          放大
        </button>
        <span className="toolbar-divider" aria-hidden="true" />
        <button
          type="button"
          aria-pressed={zoomMode === "width"}
          onClick={() => onFit("width")}
        >
          适合宽度
        </button>
        <button
          type="button"
          aria-pressed={zoomMode === "page"}
          onClick={() => onFit("page")}
        >
          适合页面
        </button>
        <button type="button" onClick={onActualSize}>
          实际大小
        </button>
        <span className="toolbar-divider" aria-hidden="true" />
        <button type="button" onClick={onToggleFullscreen}>
          {fullscreen ? "退出全屏" : "全屏"}
        </button>
      </div>
    </div>
  );
}

export function ThumbnailRail({
  documentId,
  pageCount,
  currentPageIndex,
  onSelectPage,
  canReorder = false,
  onMovePage,
}: {
  documentId: string;
  pageCount: number;
  currentPageIndex: number;
  onSelectPage: (index: number) => void;
  /** Permission Restrictions allow changing the page arrangement. */
  canReorder?: boolean;
  onMovePage?: (fromIndex: number, toIndex: number) => void;
}) {
  const railRef = useRef<HTMLElement>(null);
  const cacheRef = useRef(new Map<number, PageImageView>());
  const pendingRef = useRef(new Set<number>());
  const pageGenerationRef = useRef(new Map<number, number>());
  // A structure change renumbers every slot, so a thumbnail request that was
  // already in flight must not fill the cleared cache with stale pixels.
  const structureGenerationRef = useRef(0);
  const aliveRef = useRef(true);
  const revealedRef = useRef(currentPageIndex);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(
    THUMBNAIL_FALLBACK_VIEWPORT,
  );
  const [contentRevision, setContentRevision] = useState(0);
  const [dragFrom, setDragFrom] = useState<number | null>(null);
  const [dragOver, setDragOver] = useState<number | null>(null);
  const [, bump] = useReducer((value: number) => value + 1, 0);

  useEffect(() => {
    aliveRef.current = true;
    structureGenerationRef.current += 1;
    cacheRef.current.clear();
    pendingRef.current.clear();
    pageGenerationRef.current.clear();
    revealedRef.current = 0;
    setScrollTop(0);
    setContentRevision(0);
    return () => {
      aliveRef.current = false;
    };
  }, [documentId]);

  // An edit, delete or undo invalidates that page's thumbnail. The generation
  // check keeps a render that was already in flight from caching stale pixels.
  // A page operation renumbers the slots, so every cached thumbnail is stale.
  useEffect(
    () =>
      onDocumentChanged((detail) => {
        if (detail.documentId !== documentId) {
          return;
        }
        if (detail.pageStructureChanged) {
          structureGenerationRef.current += 1;
          cacheRef.current.clear();
          pendingRef.current.clear();
          pageGenerationRef.current.clear();
          setContentRevision((value) => value + 1);
          return;
        }
        pageGenerationRef.current.set(
          detail.pageIndex,
          (pageGenerationRef.current.get(detail.pageIndex) ?? 0) + 1,
        );
        cacheRef.current.delete(detail.pageIndex);
        pendingRef.current.delete(detail.pageIndex);
        setContentRevision((value) => value + 1);
      }),
    [documentId],
  );

  // Measure the rail after layout so the first lazy window matches its real
  // height instead of the fallback used before the first scroll.
  useLayoutEffect(() => {
    const rail = railRef.current;
    if (!rail) {
      return;
    }
    const measure = () => {
      if (rail.clientHeight > 0) {
        setViewportHeight(rail.clientHeight);
      }
    };
    measure();
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, [documentId]);

  // Navigation outside the rail (page input, Home/End, menu) scrolls the
  // active thumbnail into view; pages are still rendered only when nearby.
  useLayoutEffect(() => {
    if (revealedRef.current === currentPageIndex) {
      return;
    }
    revealedRef.current = currentPageIndex;
    const rail = railRef.current;
    if (!rail || rail.clientHeight <= 0) {
      return;
    }
    const top = currentPageIndex * THUMBNAIL_ITEM_HEIGHT;
    const bottom = top + THUMBNAIL_ITEM_HEIGHT;
    if (top < rail.scrollTop) {
      rail.scrollTop = top;
    } else if (bottom > rail.scrollTop + rail.clientHeight) {
      rail.scrollTop = bottom - rail.clientHeight;
    } else {
      return;
    }
    setScrollTop(rail.scrollTop);
  }, [currentPageIndex]);

  const handleScroll = () => {
    const rail = railRef.current;
    if (!rail) {
      return;
    }
    setScrollTop(rail.scrollTop);
    if (rail.clientHeight > 0) {
      setViewportHeight(rail.clientHeight);
    }
  };

  const range = thumbnailRange(scrollTop, viewportHeight, pageCount);

  useEffect(() => {
    for (let index = range.start; index < range.end; index += 1) {
      if (cacheRef.current.has(index) || pendingRef.current.has(index)) {
        continue;
      }
      pendingRef.current.add(index);
      const generation = pageGenerationRef.current.get(index) ?? 0;
      const structureGeneration = structureGenerationRef.current;
      renderThumbnail(documentId, index, THUMBNAIL_MAX_EDGE)
        .then((image) => {
          if (
            !aliveRef.current ||
            structureGenerationRef.current !== structureGeneration ||
            (pageGenerationRef.current.get(index) ?? 0) !== generation
          ) {
            return;
          }
          cacheRef.current.set(index, image);
          bump();
        })
        .catch((error) => {
          console.warn(`[pdfx] could not render thumbnail ${index}`, error);
        })
        .finally(() => {
          if (structureGenerationRef.current === structureGeneration) {
            pendingRef.current.delete(index);
          }
        });
    }
  }, [documentId, range.start, range.end, contentRevision]);

  const handleDragStart = (
    event: ReactDragEvent<HTMLButtonElement>,
    index: number,
  ) => {
    if (!canReorder) {
      return;
    }
    setDragFrom(index);
    setDragOver(index);
    if (event.dataTransfer) {
      event.dataTransfer.effectAllowed = "move";
      event.dataTransfer.setData("text/plain", String(index));
    }
  };

  const handleDragOver = (
    event: ReactDragEvent<HTMLButtonElement>,
    index: number,
  ) => {
    if (!canReorder || dragFrom === null) {
      return;
    }
    event.preventDefault();
    if (event.dataTransfer) {
      event.dataTransfer.dropEffect = "move";
    }
    if (dragOver !== index) {
      setDragOver(index);
    }
  };

  const handleDrop = (
    event: ReactDragEvent<HTMLButtonElement>,
    index: number,
  ) => {
    event.preventDefault();
    const from = dragFrom;
    setDragFrom(null);
    setDragOver(null);
    if (from === null || from === index) {
      return;
    }
    onMovePage?.(from, index);
  };

  const handleDragEnd = () => {
    setDragFrom(null);
    setDragOver(null);
  };

  const items = [];
  for (let index = range.start; index < range.end; index += 1) {
    const image = cacheRef.current.get(index);
    const dragging = dragFrom === index;
    const dropTarget = dragFrom !== null && dragOver === index && !dragging;
    items.push(
      <button
        key={index}
        type="button"
        className={[
          "thumbnail",
          dragging ? "thumbnail--dragging" : "",
          dropTarget ? "thumbnail--drop-target" : "",
        ]
          .filter(Boolean)
          .join(" ")}
        aria-label={`第 ${index + 1} 页`}
        aria-current={index === currentPageIndex ? "page" : undefined}
        title={canReorder ? "拖动以调整页面顺序" : undefined}
        draggable={canReorder}
        style={{
          top: `${index * THUMBNAIL_ITEM_HEIGHT}px`,
          height: `${THUMBNAIL_ITEM_HEIGHT - 8}px`,
        }}
        onClick={() => onSelectPage(index)}
        onDragStart={(event) => handleDragStart(event, index)}
        onDragOver={(event) => handleDragOver(event, index)}
        onDrop={(event) => handleDrop(event, index)}
        onDragEnd={handleDragEnd}
      >
        <span className="thumbnail-frame">
          {image ? (
            <img
              src={`data:image/png;base64,${image.pngBase64}`}
              alt=""
              draggable={false}
            />
          ) : (
            <span className="thumbnail-placeholder" aria-hidden="true" />
          )}
        </span>
        <span className="thumbnail-number">{index + 1}</span>
      </button>,
    );
  }

  return (
    <nav
      ref={railRef}
      className="thumbnail-rail"
      aria-label="页面缩略图"
      onScroll={handleScroll}
    >
      <div
        className="thumbnail-track"
        style={{ height: `${pageCount * THUMBNAIL_ITEM_HEIGHT}px` }}
      >
        {items}
      </div>
    </nav>
  );
}

export function PageSurface({
  documentId,
  pageIndex,
  info,
  scale,
  render,
  readOnly,
  canAnnotate,
  annotations,
  selectedAnnotation,
  armedStamp,
  onSelectAnnotation,
  onStampPlaced,
  onArmedStampChange,
  onAnnotationsChanged,
  surfaceRef,
  canFillForms = false,
  canModifyContent = false,
  formFocusFieldId = null,
  onFormFocusHandled,
  onSelectPage,
  onFormFocusField,
  links = [],
  onLinksChanged,
  pageCount = 0,
}: {
  documentId: string;
  pageIndex: number;
  info: PageInfo | null;
  scale: number;
  render: PageRender;
  readOnly: boolean;
  canAnnotate: boolean;
  annotations: AnnotationView[];
  selectedAnnotation: AnnotationView | null;
  armedStamp: StampKind | null;
  onSelectAnnotation: (annotation: AnnotationView | null) => void;
  onStampPlaced: (annotation: AnnotationView) => void;
  onArmedStampChange: (kind: StampKind | null) => void;
  onAnnotationsChanged: () => void;
  surfaceRef: RefObject<HTMLDivElement | null>;
  /** Permission Restrictions allow filling existing form fields. */
  canFillForms?: boolean;
  /**
   * Permission Restrictions allow changing page content: form field
   * structure, lines and arrows, and other content-object edits.
   */
  canModifyContent?: boolean;
  /** Widget to focus after the caller navigated to its page. */
  formFocusFieldId?: string | null;
  onFormFocusHandled?: () => void;
  onSelectPage?: (pageIndex: number) => void;
  onFormFocusField?: (fieldId: string) => void;
  /** Every native link in the document; the LinkLayer filters this page. */
  links?: LinkView[];
  onLinksChanged?: () => void;
  /** Number of pages an internal link may address. */
  pageCount?: number;
}) {
  const pixel = info ? pagePixelSize(info, scale) : null;
  const ready = pixel !== null && render.scale === scale;

  return (
    <div
      ref={surfaceRef}
      className="page-surface"
      role="img"
      aria-label={`第 ${pageIndex + 1} 页`}
      data-page-index={pageIndex}
      data-page-width={info?.width}
      data-page-height={info?.height}
      data-page-rotation={info?.rotation}
      data-render-mode={
        ready ? (render.image ? "full" : "tiles") : "pending"
      }
      style={
        pixel
          ? { width: `${pixel.width}px`, height: `${pixel.height}px` }
          : undefined
      }
    >
      {ready && render.image && (
        <img
          className="page-image"
          src={`data:image/png;base64,${render.image.pngBase64}`}
          alt=""
          draggable={false}
          width={render.image.width}
          height={render.image.height}
          style={{ width: "100%", height: "100%" }}
        />
      )}
      {ready &&
        render.regions.map((region: PageRegionView) => (
          <img
            key={`${region.x}:${region.y}:${region.width}:${region.height}`}
            className="page-region"
            src={`data:image/png;base64,${region.pngBase64}`}
            alt=""
            draggable={false}
            style={{
              left: `${region.x}px`,
              top: `${region.y}px`,
              width: `${region.width}px`,
              height: `${region.height}px`,
            }}
          />
        ))}
      {/* Text geometry is page-relative, so the overlay always spans the
          full page surface even when the bitmap itself is rendered as a
          viewport-sized tile at high zoom. */}
      <PageTextLayer
        key={`${documentId}:${pageIndex}`}
        documentId={documentId}
        pageIndex={pageIndex}
        readOnly={readOnly}
        canAnnotate={canAnnotate}
        annotations={annotations}
        selectedAnnotation={selectedAnnotation}
        onSelectAnnotation={onSelectAnnotation}
        scale={scale}
        pageCount={pageCount}
      />
      {info && (
        <>
          <PageImageLayer
            key={`${documentId}:${pageIndex}:images`}
            documentId={documentId}
            pageIndex={pageIndex}
            pageWidth={info.width}
            pageHeight={info.height}
            readOnly={readOnly}
            canAnnotate={canAnnotate}
            pageCount={pageCount}
          />
          <FormLayer
            documentId={documentId}
            pageIndex={pageIndex}
            pageWidth={info.width}
            pageHeight={info.height}
            scale={scale}
            canFillForms={canFillForms}
            canModifyContent={canModifyContent}
            focusFieldId={formFocusFieldId}
            onFocusHandled={onFormFocusHandled}
            onSelectPage={onSelectPage}
            onFocusField={onFormFocusField}
          />
        </>
      )}
      {info && (
        <ShapeLayer
          documentId={documentId}
          pageIndex={pageIndex}
          pageWidth={info.width}
          pageHeight={info.height}
          scale={scale}
          readOnly={readOnly}
          canAnnotate={canAnnotate}
          annotations={annotations}
          selectedAnnotation={selectedAnnotation}
          onSelectAnnotation={onSelectAnnotation}
        />
      )}
      {info && (
        <FreeTextLayer
          documentId={documentId}
          pageIndex={pageIndex}
          pageWidth={info.width}
          pageHeight={info.height}
          readOnly={readOnly}
          canAnnotate={canAnnotate}
          annotations={annotations}
          selectedAnnotation={selectedAnnotation}
          onSelectAnnotation={onSelectAnnotation}
        />
      )}
      {info && (
        <StampLayer
          documentId={documentId}
          pageIndex={pageIndex}
          pageWidth={info.width}
          pageHeight={info.height}
          readOnly={readOnly}
          canAnnotate={canAnnotate}
          annotations={annotations}
          selectedName={selectedAnnotation?.name ?? null}
          armed={armedStamp}
          onSelect={onSelectAnnotation}
          onPlaced={onStampPlaced}
          onArmedChange={onArmedStampChange}
          onChanged={onAnnotationsChanged}
        />
      )}
      {info && (
        <LineLayer
          key={`${documentId}:${pageIndex}:lines`}
          documentId={documentId}
          pageIndex={pageIndex}
          pageWidth={info.width}
          pageHeight={info.height}
          scale={scale}
          readOnly={readOnly}
          canModifyContent={canModifyContent}
        />
      )}
      {info && (
        <EraserLayer
          documentId={documentId}
          pageIndex={pageIndex}
          pageWidth={info.width}
          pageHeight={info.height}
          readOnly={readOnly}
          canAnnotate={canAnnotate}
          annotations={annotations}
          selectedName={selectedAnnotation?.name ?? null}
          onSelectAnnotation={onSelectAnnotation}
          onChanged={onAnnotationsChanged}
        />
      )}
      {info && (
        <LinkLayer
          documentId={documentId}
          pageIndex={pageIndex}
          pageWidth={info.width}
          pageHeight={info.height}
          pageCount={pageCount}
          readOnly={readOnly}
          canAnnotate={canAnnotate}
          links={links}
          onLinksChanged={() => onLinksChanged?.()}
        />
      )}
    </div>
  );
}
