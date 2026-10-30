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
import { PathShapeLayer } from "./PathShapeLayer";
import { EraserLayer } from "./EraserLayer";
import { WorkspaceIcon } from "./WorkspaceIcon";
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
  itemHeight = THUMBNAIL_ITEM_HEIGHT,
): ThumbnailRange {
  const first = Math.floor(scrollTop / itemHeight);
  const visible = Math.ceil(viewportHeight / itemHeight);
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
  onSelectPage,
  onZoomTo,
  onZoomIn,
  onZoomOut,
  onFit,
  onActualSize,
  onSearch,
}: {
  pageIndex: number;
  pageCount: number;
  scale: number;
  zoomMode: ZoomMode;
  onSelectPage: (index: number) => void;
  onZoomTo: (scale: number) => void;
  onZoomIn: () => void;
  onZoomOut: () => void;
  onFit: (mode: FitMode) => void;
  onActualSize: () => void;
  onSearch?: () => void;
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
          className="bar-icon"
          aria-label="上一页"
          title="上一页"
          disabled={pageIndex <= 0}
          onClick={() => onSelectPage(pageIndex - 1)}
        >
          <WorkspaceIcon name="chevronLeft" />
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
          className="bar-icon"
          aria-label="下一页"
          title="下一页"
          disabled={pageIndex >= pageCount - 1}
          onClick={() => onSelectPage(pageIndex + 1)}
        >
          <WorkspaceIcon name="chevronRight" />
        </button>
      </div>

      <span className="bar-divider" aria-hidden="true" />

      <div className="zoom-controls" role="group" aria-label="缩放">
        <button
          type="button"
          className="bar-icon"
          aria-label="缩小"
          title="缩小"
          onClick={onZoomOut}
        >
          <WorkspaceIcon name="zoomOut" />
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
        <button
          type="button"
          className="bar-icon"
          aria-label="放大"
          title="放大"
          onClick={onZoomIn}
        >
          <WorkspaceIcon name="zoomIn" />
        </button>
      </div>

      <span className="bar-divider" aria-hidden="true" />

      <button
        type="button"
        className="bar-icon"
        aria-label="适合宽度"
        title="适合宽度"
        aria-pressed={zoomMode === "width"}
        onClick={() => onFit("width")}
      >
        <WorkspaceIcon name="fitWidth" />
      </button>
      <button
        type="button"
        className="bar-icon"
        aria-label="适合页面"
        title="适合页面"
        aria-pressed={zoomMode === "page"}
        onClick={() => onFit("page")}
      >
        <WorkspaceIcon name="fitPage" />
      </button>
      <button
        type="button"
        className="bar-text"
        aria-label="实际大小"
        title="实际大小"
        onClick={onActualSize}
      >
        1:1
      </button>
      {onSearch && <button type="button" className="bar-icon" aria-label="搜索" title="搜索" onClick={onSearch}><WorkspaceIcon name="search" /></button>}
    </div>
  );
}

/**
 * Page-arrangement actions for the current page. They live in the workspace
 * inspector so the document toolbar stays a compact page/zoom control.
 */
export function PageOperationsPanel({
  pageCount,
  canModifyPages = false,
  pageOperationDisabledReason = null,
  onRotatePage,
  onCopyPage,
  onDeletePage,
  onInsertPage,
}: {
  pageCount: number;
  /** Permission Restrictions allow changing the page arrangement. */
  canModifyPages?: boolean;
  /** Chinese explanation shown while page operations are unavailable. */
  pageOperationDisabledReason?: string | null;
  onRotatePage?: (deltaDegrees: 90 | 180 | 270) => void;
  onCopyPage?: () => void;
  onDeletePage?: () => void;
  onInsertPage?: () => void;
}) {
  return (
    <section className="page-ops-panel" aria-label="页面操作">
      <p className="panel-hint">对当前页执行以下操作，均可通过撤销恢复。</p>
      <div className="panel-action-list" role="group" aria-label="页面操作">
        <button
          type="button"
          className="panel-action"
          disabled={!canModifyPages}
          onClick={() => onRotatePage?.(270)}
        >
          <WorkspaceIcon name="rotateLeft" />
          <span>向左旋转</span>
        </button>
        <button
          type="button"
          className="panel-action"
          disabled={!canModifyPages}
          onClick={() => onRotatePage?.(90)}
        >
          <WorkspaceIcon name="rotateRight" />
          <span>向右旋转</span>
        </button>
        <button
          type="button"
          className="panel-action"
          disabled={!canModifyPages}
          onClick={() => onInsertPage?.()}
        >
          <span>插入空白页…</span>
        </button>
        <button
          type="button"
          className="panel-action"
          title="复制当前页并插入到其后（可撤销）"
          disabled={!canModifyPages}
          onClick={() => onCopyPage?.()}
        >
          <span>复制页面</span>
        </button>
        <button
          type="button"
          className="panel-action panel-action--danger"
          disabled={!canModifyPages || pageCount <= 1}
          title={
            pageCount <= 1 ? "文档至少需要保留一页" : "删除当前页（可撤销）"
          }
          onClick={() => onDeletePage?.()}
        >
          <span>删除页面</span>
        </button>
      </div>
      {pageOperationDisabledReason && (
        <p className="panel-hint panel-hint--warning">
          {pageOperationDisabledReason}
        </p>
      )}
    </section>
  );
}

export function ThumbnailRail({
  documentId,
  pageCount,
  currentPageIndex,
  onSelectPage,
  canReorder = false,
  onMovePage,
  thumbnailSize,
}: {
  documentId: string;
  pageCount: number;
  currentPageIndex: number;
  onSelectPage: (index: number) => void;
  /** Permission Restrictions allow changing the page arrangement. */
  thumbnailSize?: number;
  canReorder?: boolean;
  onMovePage?: (fromIndex: number, toIndex: number) => void;
}) {
  const itemHeight = thumbnailSize === undefined ? THUMBNAIL_ITEM_HEIGHT : thumbnailSize + 76;
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
        if (detail.pageStructureChanged || detail.allPagesChanged) {
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
    const top = currentPageIndex * itemHeight;
    const bottom = top + itemHeight;
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

  const range = thumbnailRange(scrollTop, viewportHeight, pageCount, itemHeight);

  useEffect(() => {
    for (let index = range.start; index < range.end; index += 1) {
      if (cacheRef.current.has(index) || pendingRef.current.has(index)) {
        continue;
      }
      pendingRef.current.add(index);
      const generation = pageGenerationRef.current.get(index) ?? 0;
      const structureGeneration = structureGenerationRef.current;
      renderThumbnail(documentId, index, Math.max(THUMBNAIL_MAX_EDGE, thumbnailSize ?? 144))
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
          top: `${index * itemHeight}px`,
          height: `${itemHeight - 8}px`,
        }}
        onClick={() => onSelectPage(index)}
        onDragStart={(event) => handleDragStart(event, index)}
        onDragOver={(event) => handleDragOver(event, index)}
        onDrop={(event) => handleDrop(event, index)}
        onDragEnd={handleDragEnd}
      >
        <span className="thumbnail-frame" style={{height:thumbnailSize, maxWidth:thumbnailSize}}>
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
        style={{ height: `${pageCount * itemHeight}px` }}
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
  const [paintVersion, pagePainted] = useReducer((version: number) => version + 1, 0);
  const pixel = info ? pagePixelSize(info, scale) : null;
  const matchesPage = (view: PageImageView | PageRegionView) =>
    view.documentId === documentId && view.pageIndex === pageIndex;
  const image = render.image && matchesPage(render.image) ? render.image : null;
  const regions = render.regions.filter(matchesPage);
  const ready = pixel !== null && (image !== null || regions.length > 0);
  // Old full-page images and tiles stay painted during zoom. A tile's own
  // render scale, rather than the requested density, defines its coordinates.

  return (
    <div
      ref={surfaceRef}
      className="page-surface"
      role="region"
      aria-label={`第 ${pageIndex + 1} 页`}
      data-page-index={pageIndex}
      data-page-width={info?.width}
      data-page-height={info?.height}
      data-page-rotation={info?.rotation}
      data-render-mode={
        ready ? (render.scale !== scale ? "preview" : regions.length ? "tiles" : "full") : "pending"
      }
      style={
        pixel
          ? { width: `${pixel.width}px`, height: `${pixel.height}px` }
          : undefined
      }
    >
      {ready && image && (
        <img
          className="page-image"
          onLoad={pagePainted}
          src={`data:image/png;base64,${image.pngBase64}`}
          alt=""
          draggable={false}
          width={image.width}
          height={image.height}
          style={{ width: "100%", height: "100%" }}
        />
      )}
      {ready &&
        regions.map((region: PageRegionView) => (
          <img
            key={`${region.x}:${region.y}:${region.width}:${region.height}`}
            className="page-region"
            onLoad={pagePainted}
            src={`data:image/png;base64,${region.pngBase64}`}
            alt=""
            draggable={false}
            style={{
              left: `${region.x * scale / region.scale}px`,
              top: `${region.y * scale / region.scale}px`,
              width: `${region.width * scale / region.scale}px`,
              height: `${region.height * scale / region.scale}px`,
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
        paintVersion={paintVersion}
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
      {info && <PathShapeLayer documentId={documentId} pageIndex={pageIndex} pageWidth={info.width} pageHeight={info.height} enabled={canAnnotate && !readOnly} />}
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
