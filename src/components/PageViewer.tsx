import {
  useEffect,
  useLayoutEffect,
  useReducer,
  useRef,
  useState,
  type RefObject,
} from "react";

import {
  renderThumbnail,
  type AnnotationView,
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
import { FormLayer } from "./FormLayer";
import { LineLayer } from "./LineLayer";
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
}: {
  documentId: string;
  pageCount: number;
  currentPageIndex: number;
  onSelectPage: (index: number) => void;
}) {
  const railRef = useRef<HTMLElement>(null);
  const cacheRef = useRef(new Map<number, PageImageView>());
  const pendingRef = useRef(new Set<number>());
  const pageGenerationRef = useRef(new Map<number, number>());
  const aliveRef = useRef(true);
  const revealedRef = useRef(currentPageIndex);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(
    THUMBNAIL_FALLBACK_VIEWPORT,
  );
  const [contentRevision, setContentRevision] = useState(0);
  const [, bump] = useReducer((value: number) => value + 1, 0);

  useEffect(() => {
    aliveRef.current = true;
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
  useEffect(
    () =>
      onDocumentChanged((detail) => {
        if (detail.documentId !== documentId) {
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
      renderThumbnail(documentId, index, THUMBNAIL_MAX_EDGE)
        .then((image) => {
          if (
            !aliveRef.current ||
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
          pendingRef.current.delete(index);
        });
    }
  }, [documentId, range.start, range.end, contentRevision]);

  const items = [];
  for (let index = range.start; index < range.end; index += 1) {
    const image = cacheRef.current.get(index);
    items.push(
      <button
        key={index}
        type="button"
        className="thumbnail"
        aria-label={`第 ${index + 1} 页`}
        aria-current={index === currentPageIndex ? "page" : undefined}
        style={{
          top: `${index * THUMBNAIL_ITEM_HEIGHT}px`,
          height: `${THUMBNAIL_ITEM_HEIGHT - 8}px`,
        }}
        onClick={() => onSelectPage(index)}
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
    </div>
  );
}
