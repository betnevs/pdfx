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
  anchoredScroll,
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
import { onDocumentChanged } from "./documentEvents";
import { isEditableTarget } from "./keyboard";

/** Extra page pixels rendered around the viewport at high zoom. */
const REGION_OVERSCAN = 200;

export type ZoomMode = "custom" | "width" | "page";

export interface PageRender {
  /** Scale of the pixels in `image`/`regions`; older renders are ignored. */
  scale: number;
  image: PageImageView | null;
  regions: PageRegionView[];
}

const EMPTY_RENDER: PageRender = { scale: 0, image: null, regions: [] };

interface Anchor {
  pageScrollLeft: number;
  pageScrollTop: number;
  viewportWidth: number;
  viewportHeight: number;
  previousScale: number;
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
  zoomTo: (scale: number) => void;
  zoomBy: (factor: number) => void;
  fit: (mode: FitMode) => void;
  actualSize: () => void;
  toggleFullscreen: () => void;
  handleScroll: () => void;
  handleViewportKeyDown: (event: ReactKeyboardEvent<HTMLElement>) => void;
}

function readViewport(stage: HTMLElement | null): ScrollViewport {
  if (!stage) {
    return { scrollLeft: 0, scrollTop: 0, width: 0, height: 0 };
  }
  return {
    scrollLeft: stage.scrollLeft,
    scrollTop: stage.scrollTop,
    width: stage.clientWidth,
    height: stage.clientHeight,
  };
}

function rectKey(rect: Rect): string {
  return `${rect.x}:${rect.y}:${rect.width}:${rect.height}`;
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
): DocumentViewModel {
  const documentId = document?.documentId ?? null;

  const [pageIndex, setPageIndex] = useState(0);
  const [scale, setScale] = useState(DEFAULT_SCALE);
  const [zoomMode, setZoomMode] = useState<ZoomMode>("custom");
  const [infos, setInfos] = useState<Record<number, PageInfo>>({});
  const [render, setRender] = useState<PageRender>(EMPTY_RENDER);
  const [fullscreen, setFullscreenState] = useState(false);
  // Bumped when an edit changes page content; re-renders the same page.
  const [contentRevision, setContentRevision] = useState(0);

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

  const stateRef = useRef({
    document,
    pageIndex,
    scale,
    zoomMode,
    info,
    render,
  });
  stateRef.current = { document, pageIndex, scale, zoomMode, info, render };
  fullscreenRef.current = fullscreen;

  // A new document starts on page one at actual size.
  useEffect(() => {
    requestRef.current += 1;
    regionKeyRef.current = null;
    anchorRef.current = null;
    fullscreenSnapshotRef.current = null;
    setPageIndex(0);
    setScale(DEFAULT_SCALE);
    setZoomMode("custom");
    setInfos({});
    setRender(EMPTY_RENDER);
    setFullscreenState(false);
  }, [documentId]);

  // Edits, deletions, undo and redo change the page bitmap without changing
  // page geometry, so only the render effect needs to run again.
  useEffect(() => {
    if (!documentId) {
      return;
    }
    return onDocumentChanged((detail) => {
      if (detail.documentId === documentId) {
        setContentRevision((value) => value + 1);
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
  useEffect(() => {
    if (!documentId || !info) {
      return;
    }
    const requestId = ++requestRef.current;
    regionKeyRef.current = null;
    let cancelled = false;

    setRender({ scale, image: null, regions: [] });

    if (shouldTile(info, scale)) {
      const pixel = pagePixelSize(info, scale);
      const region = visiblePageRegion(
        readViewport(viewportRef.current),
        pixel,
        REGION_OVERSCAN,
      );
      if (region.width <= 0 || region.height <= 0) {
        return;
      }
      regionKeyRef.current = rectKey(region);
      renderPageRegion(
        documentId,
        pageIndex,
        scale,
        region.x,
        region.y,
        region.width,
        region.height,
      )
        .then((view) => {
          if (!cancelled && requestRef.current === requestId) {
            setRender({ scale, image: null, regions: [view] });
          }
        })
        .catch((error) => {
          if (!cancelled && requestRef.current === requestId) {
            onErrorRef.current(toCommandError(error));
          }
        });
    } else {
      renderPage(documentId, pageIndex, scale)
        .then((image) => {
          if (!cancelled && requestRef.current === requestId) {
            setRender({ scale, image, regions: [] });
          }
        })
        .catch((error) => {
          if (!cancelled && requestRef.current === requestId) {
            onErrorRef.current(toCommandError(error));
          }
        });
    }

    return () => {
      cancelled = true;
    };
  }, [documentId, pageIndex, scale, info, contentRevision]);

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
        { width: stage.clientWidth, height: stage.clientHeight },
        info,
      );
      if (next !== stateRef.current.scale) {
        setScale(next);
      }
    };
    recompute();
    window.addEventListener("resize", recompute);
    return () => window.removeEventListener("resize", recompute);
  }, [info, zoomMode]);

  // Keep the page point at the viewport centre fixed while zooming.
  useLayoutEffect(() => {
    const anchor = anchorRef.current;
    if (!anchor) {
      return;
    }
    anchorRef.current = null;
    const stage = viewportRef.current;
    const surface = surfaceRef.current;
    if (!stage || !surface) {
      return;
    }
    const pageScrollLeft = anchoredScroll(
      anchor.pageScrollLeft,
      anchor.viewportWidth,
      anchor.previousScale,
      scale,
    );
    const pageScrollTop = anchoredScroll(
      anchor.pageScrollTop,
      anchor.viewportHeight,
      anchor.previousScale,
      scale,
    );
    stage.scrollLeft = pageScrollLeft + surface.offsetLeft;
    stage.scrollTop = pageScrollTop + surface.offsetTop;
  }, [scale]);

  // Navigating to another page starts at its top-left corner.
  useLayoutEffect(() => {
    const stage = viewportRef.current;
    if (stage) {
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

  const goToPage = useCallback((index: number) => {
    const current = stateRef.current;
    if (!current.document || !Number.isFinite(index)) {
      return;
    }
    const next = Math.min(
      current.document.pageCount - 1,
      Math.max(0, Math.trunc(index)),
    );
    if (next === current.pageIndex) {
      return;
    }
    anchorRef.current = null;
    setPageIndex(next);
  }, []);

  const zoomTo = useCallback((requested: number) => {
    const next = clampScale(requested);
    const current = stateRef.current;
    if (!current.document || next === current.scale) {
      return;
    }
    const stage = viewportRef.current;
    const surface = surfaceRef.current;
    if (stage && surface && stage.clientWidth > 0 && stage.clientHeight > 0) {
      anchorRef.current = {
        pageScrollLeft: stage.scrollLeft - surface.offsetLeft,
        pageScrollTop: stage.scrollTop - surface.offsetTop,
        viewportWidth: stage.clientWidth,
        viewportHeight: stage.clientHeight,
        previousScale: current.scale,
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
      stage.scrollTop = 0;
    }
    if (!current.info) {
      return;
    }
    const next = fitScale(
      mode,
      stage
        ? { width: stage.clientWidth, height: stage.clientHeight }
        : { width: 0, height: 0 },
      current.info,
    );
    if (next !== current.scale) {
      setScale(next);
    }
  }, []);

  const actualSize = useCallback(() => {
    zoomToCustom(DEFAULT_SCALE);
  }, [zoomToCustom]);

  const handleScroll = useCallback(() => {
    const current = stateRef.current;
    const { document: currentDocument, info: currentInfo, scale: currentScale } =
      current;
    if (
      !currentDocument ||
      !currentInfo ||
      !shouldTile(currentInfo, currentScale)
    ) {
      return;
    }
    const stage = viewportRef.current;
    if (!stage) {
      return;
    }
    const pixel = pagePixelSize(currentInfo, currentScale);
    const region = visiblePageRegion(
      readViewport(stage),
      pixel,
      REGION_OVERSCAN,
    );
    if (region.width <= 0 || region.height <= 0) {
      return;
    }
    const rendered = current.render;
    if (
      rendered.scale === currentScale &&
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
    const requestedScale = currentScale;
    renderPageRegion(
      currentDocumentId,
      currentPageIndex,
      requestedScale,
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
          latest.scale === requestedScale
        ) {
          setRender({
            scale: requestedScale,
            image: null,
            regions: [view],
          });
        }
      })
      .catch((error) => {
        if (requestRef.current === requestId) {
          onErrorRef.current(toCommandError(error));
        }
      });
  }, []);

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
    zoomTo,
    zoomBy,
    fit,
    actualSize,
    toggleFullscreen,
    handleScroll,
    handleViewportKeyDown,
  };
}
