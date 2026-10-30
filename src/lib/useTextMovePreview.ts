import { useEffect, useState } from "react";
import { prepareTextBoxMovePreview, type TextBoxTargetView, type TextBoxMovePreview } from "./backend";

type CachedPreview = {key: string; status: "loading" | "ready" | "fallback"; data: TextBoxMovePreview | null; decoded?: HTMLImageElement[]};

/** Warm once on selection; pointer movement never reaches the native renderer. */
export function useTextMovePreview(documentId: string, pageIndex: number, box: TextBoxTargetView | null, revision: number, scale: number) {
  const key = `${documentId}:${pageIndex}:${box?.boxId}:${revision}:${scale}`;
  const [cached, setCached] = useState<CachedPreview | null>(null);
  useEffect(() => {
    if (!box) { setCached(null); return; }
    let cancelled = false;
    setCached({key, status: "loading", data: null});
    prepareTextBoxMovePreview(documentId, pageIndex, box.boxId, scale).then(async data => {
      const decoded = data
        ? await Promise.all([data.background, data.foreground].map(async item => {
          const image = new Image();
          image.src = `data:image/png;base64,${item.pngBase64}`;
          if (typeof image.decode === "function") await image.decode();
          return image;
        })) : [];
      // Keep decoded resources alive between selection and the first move.
      if (!cancelled) setCached({key, status: data ? "ready" : "fallback", data, decoded});
    }).catch(() => {
      if (!cancelled) setCached({key, status: "fallback", data: null});
    });
    return () => { cancelled = true; };
  }, [documentId, pageIndex, box, revision, scale, key]);
  return cached?.key === key ? cached : {key, status: "loading" as const, data: null};
}
