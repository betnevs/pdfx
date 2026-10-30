import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type RefObject,
} from "react";

import {
  isFullscreen,
  pageInfo,
  renderPage,
  renderPageRegion,
  setFullscreen,
  toCommandError,
  type CommandError,
  type DocumentView,
  type PageImageView,
  type PageInfo,
  type PageRegionView,
} from "./backend";
import {
  clampScale,
  DEFAULT_SCALE,
  fitScale,
  pagePixelSize,
  shouldTile,
  visiblePageRegion,
  type FitMode,
  type Rect,
  type ScrollViewport,
} from "./pageGeometry";
import { flushPendingEdits, hasPendingEdits } from "./pendingEdits";
import { onDocumentChanged } from "./documentEvents";
import { isEditableTarget } from "./keyboard";

/** Extra page pixels rendered around the viewport at high zoom. */
const REGION_OVERSCAN = 200;
/** PDFX renders at most this many bitmap pixels per CSS pixel. */
const MAX_RENDER_PIXEL_RATIO = 2;
/** Mirrors the backend's maximum scale; a render scale may not exceed it. */
const MAX_RENDER_SCALE = 40;

export type ZoomMode = "custom" | "width" | "page";

/**
 * Bitmap pixels per CSS pixel. Retina displays need a 2x bitmap or the page
 * looks blurry, but the backend rejects scales above 40x, so at extreme zoom
 * the density is reduced to whatever the zoom leaves room for.
 */
export function renderPixelRatio(scale: number, devicePixelRatio = 1): number {
  const density = Number.isFinite(devicePixelRatio) && devicePixelRatio > 0
    ? devicePixelRatio
    : 1;
  const preferred = Math.min(MAX_RENDER_PIXEL_RATIO, Math.max(1, density));
  const available = MAX_RENDER_SCALE / Math.max(scale, Number.MIN_VALUE);
  return Math.max(1, Math.min(preferred, available));
}

export interface PageRender {
  /** Scale of the pixels in `image`/`regions`; older renders are ignored. */
  scale: number;
  /** Bitmap pixels per CSS pixel used for this render. */
  pixelRatio: number;
  image: PageImageView | null;
  regions: PageRegionView[];
}

const EMPTY_RENDER: PageRender = {
  scale: 0,
  pixelRatio: 1,
  image: null,
  regions: [],
};

interface Anchor {
  pageScrollLeft: number;
  pageScrollTop: number;
  viewportWidth: number;
  viewportHeight: number;
  previousScale: number;
  pointX?: number;
  pointY?: number;
  pageIndex?: number;
}

interface FullscreenSnapshot {
  pageIndex: number;
  scale: number;
  zoomMode: ZoomMode;
}

export interface DocumentViewModel {
  pageIndex: number;
  scale: number;
  zoomMode: ZoomMode;
  info: PageInfo | null;
  render: PageRender;
  fullscreen: boolean;
  viewportRef: RefObject<HTMLDivElement | null>;
  surfaceRef: RefObject<HTMLDivElement | null>;
  goToPage: (index: number) => void;
  setVisiblePage: (index: number) => void;
  zoomTo: (scale: number) => void;
  zoomBy: (factor: number) => void;
  fit: (mode: FitMode) => void;
  actualSize: () => void;
  toggleFullscreen: () => void;
  handleScroll: () => void;
  handleViewportKeyDown: (event: ReactKeyboardEvent<HTMLElement>) => void;
}

function readViewport(stage: HTMLElement | null, surface?: HTMLElement | null): ScrollViewport {
  if (!stage) {
    return { scrollLeft: 0, scrollTop: 0, width: 0, height: 0 };
  }
  return {
    scrollLeft: surface ? stage.getBoundingClientRect().left - surface.getBoundingClientRect().left : stage.scrollLeft,
    scrollTop: surface ? stage.getBoundingClientRect().top - surface.getBoundingClientRect().top : stage.scrollTop,
    width: stage.clientWidth,
    height: stage.clientHeight,
  };
}


function fittingViewport(stage: HTMLElement | null): { width: number; height: number } {
  if (!stage) return { width: 0, height: 0 };
  const inner = stage.querySelector<HTMLElement>(".page-stage-inner");
  const style = inner ? getComputedStyle(inner) : null;
  const padding = (value: string | undefined) => Number.parseFloat(value ?? "0") || 0;
  return {
    width: Math.max(0, stage.clientWidth - padding(style?.paddingLeft) - padding(style?.paddingRight)),
    height: Math.max(0, stage.clientHeight - padding(style?.paddingTop) - padding(style?.paddingBottom)),
  };
}

function rectKey(rect: Rect): string {
  return `${rect.x}:${rect.y}:${rect.width}:${rect.height}`;
}

/** CSS-pixel viewport measurements in render-bitmap pixels. */
function scaleViewport(viewport: ScrollViewport, ratio: number): ScrollViewport {
  return {
    scrollLeft: viewport.scrollLeft * ratio,
    scrollTop: viewport.scrollTop * ratio,
    width: viewport.width * ratio,
    height: viewport.height * ratio,
  };
}

function covers(outer: Rect, target: Rect): boolean {
  return (
    outer.x <= target.x &&
    outer.y <= target.y &&
    outer.x + outer.width >= target.x + target.width &&
    outer.y + outer.height >= target.y + target.height
  );
}

/**
 * Owns the page viewer state: navigation, zoom, high-zoom tiling and native
 * fullscreen. Rendering is effect-driven so UI handlers stay synchronous and
 * stale responses are discarded by request sequence.
 */
export function useDocumentViewer(
  document: DocumentView | null,
  onError: (error: CommandError) => void,
  continuous = false,
): DocumentViewModel {
  const documentId = document?.documentId ?? null;

  const [pageIndex, setPageIndex] = useState(0);
  const [scale, setScale] = useState(DEFAULT_SCALE);
  const [zoomMode, setZoomMode] = useState<ZoomMode>(continuous ? "width" : "custom");
  const [density, setDensity] = useState(() =>
    typeof window === "undefined" ? 1 : window.devicePixelRatio || 1,
  );
  const [infos, setInfos] = useState<Record<number, PageInfo>>({});
  const [render, setRender] = useState<PageRender>(EMPTY_RENDER);
  const [fullscreen, setFullscreenState] = useState(false);
  // Bumped when an edit changes page content; re-renders the same page.
  const [contentRevision, setContentRevision] = useState(0);

  const continuousRef = useRef(continuous);
  continuousRef.current = continuous;
  const viewportRef = useRef<HTMLDivElement>(null);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const requestRef = useRef(0);
  const regionKeyRef = useRef<string | null>(null);
  const anchorRef = useRef<Anchor | null>(null);
  const fullscreenRef = useRef(false);
  const fullscreenSnapshotRef = useRef<FullscreenSnapshot | null>(null);
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;

  const info = document ? (infos[pageIndex] ?? null) : null;
  const pixelRatio = renderPixelRatio(scale, density);

  const stateRef = useRef({
    document,
    pageIndex,
    scale,
    zoomMode,
    info,
    render,
    pixelRatio,
  });
  stateRef.current = {
    document,
    pageIndex,
    scale,
    zoomMode,
    info,
    render,
    pixelRatio,
  };
  fullscreenRef.current = fullscreen;

  // A window moved to a different display changes its pixel density; render
  // again so the page stays sharp.
  useEffect(() => {
    const update = () => {
      setDensity(window.devicePixelRatio || 1);
    };
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, []);

  // The editing workspace opens at a readable width; standalone viewers retain actual size.
  useEffect(() => {
    requestRef.current += 1;
    regionKeyRef.current = null;
    anchorRef.current = null;
    fullscreenSnapshotRef.current = null;
    setPageIndex(0);
    setScale(DEFAULT_SCALE);
    setZoomMode(continuousRef.current ? "width" : "custom");
    setInfos({});
    setRender(EMPTY_RENDER);
    setFullscreenState(false);
  }, [documentId]);

  // Edits, deletions, undo and redo change the page bitmap. Page operations
  // additionally change what each slot contains (and possibly its geometry),
  // so cached metadata is dropped and the requested page becomes current.
  useEffect(() => {
    if (!documentId) {
      return;
    }
    return onDocumentChanged((detail) => {
      if (detail.documentId !== documentId) {
        return;
      }
      setContentRevision((value) => value + 1);
      if (detail.pageStructureChanged) {
        setInfos({});
        if (typeof detail.pageCount === "number") {
          const lastPage = Math.max(0, detail.pageCount - 1);
          setPageIndex(Math.min(Math.max(0, detail.pageIndex), lastPage));
        }
      }
    });
  }, [documentId]);

  // Page size and rotation matter for fitting and for overlays, so metadata
  // is fetched once per page and cached for the life of the document.
  useEffect(() => {
    if (!documentId || info) {
      return;
    }
    let cancelled = false;
    pageInfo(documentId, pageIndex)
      .then((next) => {
        if (!cancelled) {
          setInfos((current) => ({ ...current, [next.pageIndex]: next }));
        }
      })
      .catch((error) => {
        if (!cancelled) {
          onErrorRef.current(toCommandError(error));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [documentId, pageIndex, info]);

  // Render the current page after every page/zoom/metadata change. Large
  // bitmaps are replaced by a viewport-sized region to keep 4000% usable.
  // Bitmaps are rendered at the display's pixel density so they stay sharp on
  // Retina screens; all layout geometry remains in CSS pixels.
  useEffect(() => {
    if (!documentId || !info) {
      return;
    }
    const requestId = ++requestRef.current;
    regionKeyRef.current = null;
    let cancelled = false;
    const renderScale = scale * pixelRatio;

    // Retain the last painted page while a replacement is being rasterized.
    // The surface scales it to the new geometry instead of exposing white.
    const keepPageImage = (previous: PageRender) =>
      previous.image?.documentId === documentId && previous.image.pageIndex === pageIndex
        ? previous.image : null;
    const paint = () => {
      if (shouldTile(info, renderScale)) {
        const pixel = pagePixelSize(info, renderScale);
        const region = visiblePageRegion(
          scaleViewport(readViewport(viewportRef.current, continuous ? surfaceRef.current : null), pixelRatio),
          pixel,
          REGION_OVERSCAN * pixelRatio,
        );
        if (region.width <= 0 || region.height <= 0) {
          return;
        }
        regionKeyRef.current = rectKey(region);
        renderPageRegion(
          documentId,
          pageIndex,
          renderScale,
          region.x,
          region.y,
          region.width,
          region.height,
        )
          .then((view) => {
            if (!cancelled && requestRef.current === requestId) {
              setRender((previous) => ({ scale, pixelRatio, image: keepPageImage(previous), regions: [view] }));
            }
          })
          .catch((error) => {
            if (!cancelled && requestRef.current === requestId) {
              onErrorRef.current(toCommandError(error));
            }
          });
      } else {
        renderPage(documentId, pageIndex, renderScale)
          .then((image) => {
            if (!cancelled && requestRef.current === requestId) {
              setRender({ scale, pixelRatio, image, regions: [] });
            }
          })
          .catch((error) => {
            if (!cancelled && requestRef.current === requestId) {
              onErrorRef.current(toCommandError(error));
            }
          });
      }

    };
    // Wheel events can arrive faster than native rasterization. Coalesce the
    // expensive calls while keeping geometry/overlays responsive each frame.
    const previous = stateRef.current.render;
    const zooming = previous.scale > 0 && previous.scale !== scale;
    const timer = zooming ? window.setTimeout(paint, 80) : null;
    if (!zooming) paint();
    return () => {
      cancelled = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [documentId, pageIndex, scale, pixelRatio, info, contentRevision]);

  // Fit modes follow viewport and page changes until the user zooms manually.
  useEffect(() => {
    if (!info || zoomMode === "custom") {
      return;
    }
    const recompute = () => {
      const stage = viewportRef.current;
      if (!stage) {
        return;
      }
      const next = fitScale(
        zoomMode,
        fittingViewport(stage),
        info,
      );
      if (next !== stateRef.current.scale) {
        setScale(next);
      }
    };
    recompute();
    window.addEventListener("resize", recompute);
    const observer=typeof ResizeObserver === "undefined" ? null : new ResizeObserver(recompute);
    if (viewportRef.current) observer?.observe(viewportRef.current);
    return () => {window.removeEventListener("resize", recompute);observer?.disconnect();};
  }, [info, zoomMode]);

  // Keep the page point at the viewport centre fixed while zooming.
  useLayoutEffect(() => {
    const anchor = anchorRef.current;
    if (!anchor) {
      return;
    }
    anchorRef.current = null;
    const stage = viewportRef.current;
    const surface = anchor.pageIndex === undefined ? surfaceRef.current : stage?.querySelector<HTMLElement>(`[data-page-slot="${anchor.pageIndex}"]`);
    if (!stage || !surface) {
      return;
    }
    // Page-relative offsets can be negative for a centred page. Clamp only
    // after returning to viewport coordinates, or wheel zoom jumps sideways.
    const pointX = anchor.pointX ?? anchor.viewportWidth / 2;
    const pointY = anchor.pointY ?? anchor.viewportHeight / 2;
    const pageScrollLeft = (anchor.pageScrollLeft + pointX) * scale / anchor.previousScale - pointX;
    const pageScrollTop = (anchor.pageScrollTop + pointY) * scale / anchor.previousScale - pointY;
    const offsetLeft = continuousRef.current ? surface.getBoundingClientRect().left - stage.getBoundingClientRect().left + stage.scrollLeft : surface.offsetLeft;
    const offsetTop = continuousRef.current ? surface.getBoundingClientRect().top - stage.getBoundingClientRect().top + stage.scrollTop : surface.offsetTop;
    stage.scrollLeft = Math.max(0, pageScrollLeft + offsetLeft);
    stage.scrollTop = Math.max(0, pageScrollTop + offsetTop);
  }, [scale]);

  // Navigating to another page starts at its top-left corner.
  useLayoutEffect(() => {
    const stage = viewportRef.current;
    if (stage && !continuousRef.current) {
      stage.scrollLeft = 0;
      stage.scrollTop = 0;
    }
  }, [pageIndex, documentId]);

  // Reflect fullscreen if the window already starts in that state.
  useEffect(() => {
    let cancelled = false;
    isFullscreen()
      .then((value) => {
        if (!cancelled) {
          setFullscreenState(value);
        }
      })
      .catch(() => {
        // A window without fullscreen support keeps the in-app state.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const goToPage = useCallback(async (index: number) => {
    const current = stateRef.current;
    if (!current.document || !Number.isFinite(index)) {
      return;
    }
    const next = Math.min(
      current.document.pageCount - 1,
      Math.max(0, Math.trunc(index)),
    );
    if (next !== current.pageIndex && hasPendingEdits(current.document.documentId) && !(await flushPendingEdits(current.document.documentId))) return;
    if (continuousRef.current) {
      const stage = viewportRef.current;
      const target = stage?.querySelector<HTMLElement>(`[data-page-slot="${next}"]`);
      if (stage && target) stage.scrollTop += target.getBoundingClientRect().top - stage.getBoundingClientRect().top - 16;
    }
    if (next === current.pageIndex) return;
    anchorRef.current = null;
    setPageIndex(next);
  }, []);

  const setVisiblePage = useCallback(async (index: number) => {
    if (index === stateRef.current.pageIndex) return;
    const id = stateRef.current.document?.documentId;
    if (id && hasPendingEdits(id) && !(await flushPendingEdits(id))) return;
    const count = stateRef.current.document?.pageCount ?? 0;
    if (Number.isInteger(index) && index >= 0 && index < count) setPageIndex(index);
  }, []);

  const zoomTo = useCallback((requested: number, point?: {x:number;y:number;pageIndex?:number}) => {
    const next = clampScale(requested);
    const current = stateRef.current;
    if (!current.document || next === current.scale) {
      return;
    }
    const stage = viewportRef.current;
    const surface = point?.pageIndex === undefined ? surfaceRef.current : stage?.querySelector<HTMLElement>(`[data-page-slot="${point.pageIndex}"]`);
    if (stage && surface && stage.clientWidth > 0 && stage.clientHeight > 0) {
      anchorRef.current = {
        pageScrollLeft: continuousRef.current ? stage.getBoundingClientRect().left - surface.getBoundingClientRect().left : stage.scrollLeft - surface.offsetLeft,
        pageScrollTop: continuousRef.current ? stage.getBoundingClientRect().top - surface.getBoundingClientRect().top : stage.scrollTop - surface.offsetTop,
        viewportWidth: stage.clientWidth,
        viewportHeight: stage.clientHeight,
        previousScale: current.scale,
        ...(point ? {pointX:point.x-stage.getBoundingClientRect().left,pointY:point.y-stage.getBoundingClientRect().top,pageIndex:point.pageIndex} : {}),
      };
    }
    setScale(next);
  }, []);

  const zoomToCustom = useCallback(
    (requested: number) => {
      setZoomMode("custom");
      zoomTo(requested);
    },
    [zoomTo],
  );

  const zoomBy = useCallback(
    (factor: number) => {
      zoomToCustom(stateRef.current.scale * factor);
    },
    [zoomToCustom],
  );

  const fit = useCallback((mode: FitMode) => {
    const stage = viewportRef.current;
    const current = stateRef.current;
    setZoomMode(mode);
    anchorRef.current = null;
    if (stage) {
      stage.scrollLeft = 0;
      if (!continuousRef.current) stage.scrollTop = 0;
    }
    if (!current.info) {
      return;
    }
    const next = fitScale(
      mode,
      fittingViewport(stage),
      current.info,
    );
    if (next !== current.scale) {
      setScale(next);
    }
  }, []);

  const actualSize = useCallback(() => {
    zoomToCustom(DEFAULT_SCALE);
  }, [zoomToCustom]);

  useEffect(() => {
    const stage = viewportRef.current;
    if (!documentId || !stage) return;
    let frame = 0;
    let delta = 0;
    let point: { x: number; y: number; pageIndex?: number } | undefined;
    const wheel = (event: WheelEvent) => {
      if (!event.ctrlKey && !event.metaKey) return;
      event.preventDefault();
      delta += event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? stage.clientHeight : 1);
      const slot = event.target instanceof Element ? event.target.closest<HTMLElement>("[data-page-slot]") : null;
      point = { x: event.clientX, y: event.clientY, ...(slot ? { pageIndex: Number(slot.dataset.pageSlot) } : {}) };
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        const factor = Math.exp(-Math.max(-240, Math.min(240, delta)) * 0.002);
        delta = 0;
        setZoomMode("custom");
        zoomTo(stateRef.current.scale * factor, point);
      });
    };
    stage.addEventListener("wheel", wheel, { passive: false });
    return () => {
      stage.removeEventListener("wheel", wheel);
      if (frame) cancelAnimationFrame(frame);
    };
  }, [documentId, zoomTo]);

  const handleScroll = useCallback(() => {
    const stageElement = viewportRef.current;
    if (continuousRef.current && stageElement && stageElement.clientHeight > 0) {
      const targetY = stageElement.getBoundingClientRect().top + Math.min(160, stageElement.clientHeight / 3);
      const slots = stageElement.querySelectorAll<HTMLElement>("[data-page-slot]");
      for (const slot of slots) {
        const bounds = slot.getBoundingClientRect();
        if (bounds.top <= targetY && bounds.bottom > targetY) {
          void setVisiblePage(Number(slot.dataset.pageSlot));
          break;
        }
      }
    }
    const current = stateRef.current;
    const { document: currentDocument, info: currentInfo, scale: currentScale } =
      current;
    const ratio = current.pixelRatio;
    const renderScale = currentScale * ratio;
    if (!currentDocument || !currentInfo || !shouldTile(currentInfo, renderScale)) {
      return;
    }
    const stage = viewportRef.current;
    if (!stage) {
      return;
    }
    const pixel = pagePixelSize(currentInfo, renderScale);
    const region = visiblePageRegion(
      scaleViewport(readViewport(stage, continuousRef.current ? surfaceRef.current : null), ratio),
      pixel,
      REGION_OVERSCAN * ratio,
    );
    if (region.width <= 0 || region.height <= 0) {
      return;
    }
    const rendered = current.render;
    if (
      rendered.scale === currentScale &&
      rendered.pixelRatio === ratio &&
      rendered.regions.some((view) => covers(view, region))
    ) {
      return;
    }

    const key = rectKey(region);
    if (key === regionKeyRef.current) {
      return;
    }
    regionKeyRef.current = key;

    const requestId = ++requestRef.current;
    const currentDocumentId = currentDocument.documentId;
    const currentPageIndex = current.pageIndex;
    renderPageRegion(
      currentDocumentId,
      currentPageIndex,
      renderScale,
      region.x,
      region.y,
      region.width,
      region.height,
    )
      .then((view) => {
        const latest = stateRef.current;
        if (
          requestRef.current === requestId &&
          latest.document?.documentId === currentDocumentId &&
          latest.pageIndex === currentPageIndex &&
          latest.scale === currentScale &&
          latest.pixelRatio === ratio
        ) {
          setRender((previous) => ({
            scale: currentScale,
            pixelRatio: ratio,
            image: previous.image?.documentId === currentDocumentId && previous.image.pageIndex === currentPageIndex
              ? previous.image : null,
            regions: [view],
          }));
        }
      })
      .catch((error) => {
        if (requestRef.current === requestId) {
          onErrorRef.current(toCommandError(error));
        }
      });
  }, [setVisiblePage]);

  const handleViewportKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLElement>) => {
      if (
        event.defaultPrevented ||
        event.metaKey ||
        event.ctrlKey ||
        event.altKey ||
        isEditableTarget(event.target)
      ) {
        return;
      }
      const current = stateRef.current;
      if (!current.document) {
        return;
      }
      if (event.key === "Home") {
        event.preventDefault();
        goToPage(0);
      } else if (event.key === "End") {
        event.preventDefault();
        goToPage(current.document.pageCount - 1);
      }
    },
    [goToPage],
  );

  const toggleFullscreen = useCallback(() => {
    const entering = !fullscreenRef.current;
    fullscreenRef.current = entering;
    setFullscreenState(entering);

    if (entering) {
      const current = stateRef.current;
      fullscreenSnapshotRef.current = {
        pageIndex: current.pageIndex,
        scale: current.scale,
        zoomMode: current.zoomMode,
      };
    } else {
      const snapshot = fullscreenSnapshotRef.current;
      fullscreenSnapshotRef.current = null;
      if (snapshot && stateRef.current.document) {
        anchorRef.current = null;
        setZoomMode(snapshot.zoomMode);
        if (snapshot.pageIndex !== stateRef.current.pageIndex) {
          setPageIndex(snapshot.pageIndex);
        }
        if (snapshot.scale !== stateRef.current.scale) {
          setScale(snapshot.scale);
        }
      }
    }

    setFullscreen(entering).catch((error) => {
      fullscreenRef.current = !entering;
      setFullscreenState(!entering);
      console.warn("[pdfx] could not change fullscreen", error);
    });
  }, []);

  // Escape leaves fullscreen, matching the platform convention.
  useEffect(() => {
    if (!fullscreen) {
      return;
    }
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        toggleFullscreen();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [fullscreen, toggleFullscreen]);

  return {
    pageIndex,
    scale,
    zoomMode,
    info,
    render,
    fullscreen,
    viewportRef,
    surfaceRef,
    goToPage,
    setVisiblePage,
    zoomTo: zoomToCustom,
    zoomBy,
    fit,
    actualSize,
    toggleFullscreen,
    handleScroll,
    handleViewportKeyDown,
  };
}
