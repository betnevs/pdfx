import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  pageInfo,
  renderPage,
  type DocumentView,
  type PageInfo,
  type PageImageView,
} from "../lib/backend";
import { onDocumentChanged } from "../lib/documentEvents";
import { pagePixelSize } from "../lib/pageGeometry";
import type { DocumentViewModel } from "../lib/useDocumentViewer";

/** Offscreen pages keep their geometry, while only nearby bitmaps are loaded. */
function PageSlot({
  document,
  index,
  viewer,
  children,
}: {
  document: DocumentView;
  index: number;
  viewer: DocumentViewModel;
  children: ReactNode;
}) {
  const slot = useRef<HTMLDivElement>(null);
  const active = index === viewer.pageIndex;
  const [near, setNear] = useState(false);
  const [info, setInfo] = useState<PageInfo | null>(null);
  const [image, setImage] = useState<PageImageView | null>(null);
  const [revision, setRevision] = useState(0);
  const previewScale = Math.min(viewer.scale * 1.5, 1.5);
  useEffect(() => {
    if (!slot.current || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(
      (entries) => setNear(entries[0].isIntersecting),
      { root: viewer.viewportRef.current, rootMargin: "800px 0px" },
    );
    observer.observe(slot.current);
    return () => observer.disconnect();
  }, [viewer.viewportRef]);
  useEffect(
    () =>
      onDocumentChanged((change) => {
        if (
          change.documentId === document.documentId &&
          (change.pageIndex === index ||
            change.pageStructureChanged ||
            change.allPagesChanged)
        ) {
          setInfo(null);
          setImage(null);
          setRevision((r) => r + 1);
        }
      }),
    [document.documentId, index],
  );
  useEffect(() => {
    if (active && viewer.info) setInfo(viewer.info);
  }, [active, viewer.info]);
  useEffect(() => {
    if (active) return;
    if (!near) {
      setImage(null);
      return;
    }
    let disposed = false;
    pageInfo(document.documentId, index)
      .then((value) => {
        if (!disposed) setInfo(value);
      })
      .catch(() => {});
    // The active page uses the editor's full resolution/tiled render. Preview
    // pages cap bitmap scale to bound memory even at 4000% zoom.
    if (!active)
      renderPage(document.documentId, index, previewScale)
        .then((value) => {
          if (!disposed) setImage(value);
        })
        .catch(() => {});
    return () => {
      disposed = true;
    };
  }, [document.documentId, index, active, near, previewScale, revision]);
  const size = pagePixelSize(
    info ?? { width: document.pageWidth, height: document.pageHeight },
    viewer.scale,
  );
  return (
    <div
      ref={slot}
      className="continuous-page-slot"
      data-page-slot={index}
      style={{ width: size.width, height: size.height }}
      onPointerDownCapture={() => {
        if (!active) viewer.setVisiblePage(index);
      }}
    >
      {active ? (
        children
      ) : (
        <div
          className="page-preview"
          role="img"
          aria-label={`第 ${index + 1} 页`}
        >
          {image && (
            <img
              src={`data:image/png;base64,${image.pngBase64}`}
              alt=""
              draggable={false}
            />
          )}
        </div>
      )}
    </div>
  );
}

export function ContinuousPages({
  document,
  viewer,
  children,
}: {
  document: DocumentView;
  viewer: DocumentViewModel;
  children: ReactNode;
}) {
  return (
    <div className="continuous-pages">
      {Array.from({ length: document.pageCount }, (_, index) => (
        <PageSlot
          key={`${document.documentId}:${index}`}
          document={document}
          index={index}
          viewer={viewer}
        >
          {index === viewer.pageIndex ? children : null}
        </PageSlot>
      ))}
    </div>
  );
}
