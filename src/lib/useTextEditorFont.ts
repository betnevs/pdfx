import { useEffect, useRef, useState } from "react";
import { textBoxEditorFonts, type TextBoxTargetView } from "./backend";
import { previewFontFamily } from "./fontPreview";

let nextFont = 0;

/** Keep native font metrics in the textarea instead of substituting a system face. */
export function useTextEditorFont(documentId: string, pageIndex: number, target: TextBoxTargetView | null, text: string) {
  const key = target?.keepOriginalFont ? `${documentId}:${pageIndex}:${target.boxId}:${target.fontFamily}:${target.bold}:${target.italic}` : null;
  const boxId = target?.boxId;
  const fallback = previewFontFamily(target?.fontFamily ?? "Helvetica");
  const [loaded, setLoaded] = useState<{key: string; family: string} | null>(null);
  const installed = useRef<FontFace[]>([]);
  useEffect(() => () => {
    for (const face of installed.current) document.fonts.delete(face);
    installed.current = [];
  }, [key]);
  useEffect(() => {
    if (!key || boxId === undefined || typeof FontFace === "undefined" || !document.fonts) return;
    let cancelled = false;
    const timer = window.setTimeout(async () => {
      try {
        const programs = await textBoxEditorFonts(documentId, pageIndex, boxId, text);
        const faces = await Promise.all(programs.map(async (data) => {
          const name = `PDFXEditor${++nextFont}`;
          const bytes = Uint8Array.from(atob(data), c => c.charCodeAt(0));
          const face = new FontFace(name, bytes, {
            weight: target?.bold ? "700" : "400",
            style: target?.italic ? "italic" : "normal",
          });
          await face.load();
          return face;
        }));
        if (cancelled || faces.length === 0) return;
        for (const face of faces) document.fonts.add(face);
        const previous = installed.current;
        installed.current = faces;
        setLoaded({key, family: faces.map(face => `"${face.family}"`).join(", ") + `, ${fallback}`});
        // Allow React to apply the new family before releasing the old one.
        requestAnimationFrame(() => previous.forEach(face => document.fonts.delete(face)));
      } catch {
        // Unsupported fonts retain the existing preview; commits still validate coverage.
      }
    }, 80);
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [documentId, pageIndex, boxId, key, text, fallback]);
  return loaded?.key === key ? loaded.family : fallback;
}
