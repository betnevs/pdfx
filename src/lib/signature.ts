/**
 * Pure helpers for the signature dialog.
 *
 * Everything a signature produces ends up as a transparent PNG handed to the
 * `insert_signature_image` command as base64. No data URL, base64 payload or
 * signature text is logged or sent anywhere: the dialog only keeps it in
 * component state and the command payload.
 *
 * The helpers here are deliberately free of React so the drawing, typing and
 * upload rules can be tested without a real canvas.
 */

/** Internal drawing surface. Rendered into the PDF at roughly 200 DPI. */
export const SIGNATURE_CANVAS_WIDTH = 1600;
export const SIGNATURE_CANVAS_HEIGHT = 600;

/** Stroke width in internal canvas pixels. */
export const SIGNATURE_STROKE_WIDTH = 10;

/** Must match the backend's `MAX_SIGNATURE_NAME_CHARS`. */
export const SIGNATURE_MAX_NAME_CHARS = 80;

/** Must match the backend's image limits (ticket 14). */
export const SIGNATURE_MAX_FILE_BYTES = 25 * 1024 * 1024;
export const SIGNATURE_MAX_PIXELS = 20_000_000;

/** Bundled, OFL-licensed Chinese handwriting face. */
export const SIGNATURE_FONT_FAMILY =
  "'Ma Shan Zheng', 'KaiTi', 'STKaiti', cursive";

/** Labels the session uses for signatures, one per entry point. */
export const SIGNATURE_LABELS = {
  draw: "手绘签名",
  type: "输入签名",
  upload: "上传签名",
} as const;

export type SignatureTab = keyof typeof SIGNATURE_LABELS;

/** Validates the name typed on the 输入 tab. `null` means valid. */
export function validateSignatureName(raw: string): string | null {
  const name = raw.trim();
  if (name.length === 0) {
    return "请输入要签写的姓名。";
  }
  if ([...name].length > SIGNATURE_MAX_NAME_CHARS) {
    return `姓名不能超过 ${SIGNATURE_MAX_NAME_CHARS} 个字符。`;
  }
  if (/[\u0000-\u001f\u007f]/.test(name)) {
    return "姓名不能包含换行或控制字符。";
  }
  return null;
}

/** Strips the `data:image/png;base64,` prefix a canvas produces. */
export function pngDataUrlToBase64(dataUrl: string): string | null {
  const match = /^data:image\/png;base64,([A-Za-z0-9+/=]+)$/.exec(dataUrl);
  return match ? match[1] : null;
}

export interface SignaturePoint {
  x: number;
  y: number;
}

export type SignatureStroke = SignaturePoint[];

/** Appends a point unless it is within `minDistance` of the previous one. */
export function addStrokePoint(
  stroke: SignatureStroke,
  point: SignaturePoint,
  minDistance = 1.5,
): SignatureStroke {
  const last = stroke[stroke.length - 1];
  if (last && Math.hypot(point.x - last.x, point.y - last.y) < minDistance) {
    return stroke;
  }
  return [...stroke, point];
}

/** Draws strokes onto a context prepared at the given size. */
export function paintSignatureStrokes(
  ctx: CanvasRenderingContext2D,
  strokes: SignatureStroke[],
  width: number,
  height: number,
): void {
  ctx.clearRect(0, 0, width, height);
  ctx.strokeStyle = "#111111";
  ctx.fillStyle = "#111111";
  ctx.lineWidth = Math.max(2, Math.round(width / 160));
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  for (const stroke of strokes) {
    if (stroke.length === 0) {
      continue;
    }
    if (stroke.length === 1) {
      const [point] = stroke;
      ctx.beginPath();
      ctx.arc(point.x, point.y, ctx.lineWidth / 2, 0, Math.PI * 2);
      ctx.fill();
      continue;
    }
    ctx.beginPath();
    ctx.moveTo(stroke[0].x, stroke[0].y);
    for (const point of stroke.slice(1)) {
      ctx.lineTo(point.x, point.y);
    }
    ctx.stroke();
  }
}

/** The bounding box of all strokes, padded by the stroke width. */
export function drawingBounds(
  strokes: SignatureStroke[],
  width = SIGNATURE_CANVAS_WIDTH,
  height = SIGNATURE_CANVAS_HEIGHT,
): { x: number; y: number; width: number; height: number } | null {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const stroke of strokes) {
    for (const point of stroke) {
      minX = Math.min(minX, point.x);
      minY = Math.min(minY, point.y);
      maxX = Math.max(maxX, point.x);
      maxY = Math.max(maxY, point.y);
    }
  }
  if (!Number.isFinite(minX) || !Number.isFinite(minY)) {
    return null;
  }
  const padding = SIGNATURE_STROKE_WIDTH;
  const left = Math.max(0, Math.floor(minX - padding));
  const top = Math.max(0, Math.floor(minY - padding));
  const right = Math.min(width, Math.ceil(maxX + padding));
  const bottom = Math.min(height, Math.ceil(maxY + padding));
  return {
    x: left,
    y: top,
    width: Math.max(1, right - left),
    height: Math.max(1, bottom - top),
  };
}

export interface CanvasImage {
  dataUrl: string;
  base64: string;
}

/**
 * Renders the drawing tab's strokes into a tightly cropped transparent PNG.
 * Returns null when the canvas API is unavailable or the drawing is empty.
 */
export function renderDrawingPng(
  strokes: SignatureStroke[],
  width = SIGNATURE_CANVAS_WIDTH,
  height = SIGNATURE_CANVAS_HEIGHT,
): CanvasImage | null {
  const bounds = drawingBounds(strokes, width, height);
  if (!bounds) {
    return null;
  }
  const canvas = document.createElement("canvas");
  canvas.width = bounds.width;
  canvas.height = bounds.height;
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    return null;
  }
  ctx.translate(-bounds.x, -bounds.y);
  paintSignatureStrokes(ctx, strokes, width, height);
  const dataUrl = canvas.toDataURL("image/png");
  const base64 = pngDataUrlToBase64(dataUrl);
  return base64 ? { dataUrl, base64 } : null;
}

/** Chooses the largest font size that fits `text` in `maxWidth`. */
function fitFontSize(
  ctx: CanvasRenderingContext2D,
  text: string,
  maxWidth: number,
  maxHeight: number,
): number {
  let fontSize = Math.max(
    12,
    Math.min(maxHeight * 0.55, maxWidth / Math.max(1, [...text].length)),
  );
  for (let attempt = 0; attempt < 8; attempt += 1) {
    ctx.font = `${fontSize}px ${SIGNATURE_FONT_FAMILY}`;
    if (ctx.measureText(text).width <= maxWidth || fontSize <= 12) {
      break;
    }
    fontSize = Math.max(12, Math.floor(fontSize * 0.9));
  }
  return fontSize;
}

/**
 * Renders a typed name in the bundled handwriting face onto a transparent,
 * tightly cropped canvas. The glyphs become pixels; no font is embedded in the
 * PDF.
 */
export function renderTypedPng(
  name: string,
  width = SIGNATURE_CANVAS_WIDTH,
  height = SIGNATURE_CANVAS_HEIGHT,
): CanvasImage | null {
  const text = name.trim();
  if (text.length === 0) {
    return null;
  }
  const measureCanvas = document.createElement("canvas");
  measureCanvas.width = width;
  measureCanvas.height = height;
  const measureCtx = measureCanvas.getContext("2d");
  if (!measureCtx) {
    return null;
  }
  const maxWidth = width * 0.88;
  const fontSize = fitFontSize(measureCtx, text, maxWidth, height);
  measureCtx.font = `${fontSize}px ${SIGNATURE_FONT_FAMILY}`;
  const metrics = measureCtx.measureText(text);
  const textWidth = Math.max(fontSize, metrics.width);
  const cropWidth = Math.min(width, Math.ceil(textWidth + fontSize * 0.5));
  const cropHeight = Math.min(height, Math.ceil(fontSize * 1.5));

  const canvas = document.createElement("canvas");
  canvas.width = cropWidth;
  canvas.height = cropHeight;
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    return null;
  }
  ctx.font = `${fontSize}px ${SIGNATURE_FONT_FAMILY}`;
  ctx.fillStyle = "#111111";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(text, cropWidth / 2, cropHeight / 2);
  const dataUrl = canvas.toDataURL("image/png");
  const base64 = pngDataUrlToBase64(dataUrl);
  return base64 ? { dataUrl, base64 } : null;
}

/** Best-effort load of the bundled face before drawing canvas text. */
export async function ensureSignatureFont(): Promise<void> {
  const fonts = document.fonts;
  if (!fonts || typeof fonts.load !== "function") {
    return;
  }
  try {
    await fonts.load(`64px ${SIGNATURE_FONT_FAMILY}`);
  } catch {
    // A missing font falls back to the platform's cursive face.
  }
}

export interface RgbaImage {
  width: number;
  height: number;
  data: Uint8ClampedArray;
}

/** True when any pixel is not fully opaque. */
export function hasTransparency(image: RgbaImage): boolean {
  const { data } = image;
  for (let offset = 3; offset < data.length; offset += 4) {
    if (data[offset] < 250) {
      return true;
    }
  }
  return false;
}

/** Ratio of fully opaque near-white pixels, used to explain JPEG uploads. */
export function whiteBackgroundRatio(
  image: RgbaImage,
  threshold = 245,
): number {
  const { data } = image;
  const pixels = image.width * image.height;
  if (pixels === 0) {
    return 0;
  }
  let white = 0;
  for (let offset = 0; offset < data.length; offset += 4) {
    if (
      data[offset] >= threshold &&
      data[offset + 1] >= threshold &&
      data[offset + 2] >= threshold &&
      data[offset + 3] > 0
    ) {
      white += 1;
    }
  }
  return white / pixels;
}

/** True when at least one pixel is visibly inked. */
export function hasVisiblePixels(image: RgbaImage): boolean {
  const { data } = image;
  for (let offset = 3; offset < data.length; offset += 4) {
    if (data[offset] > 8) {
      return true;
    }
  }
  return false;
}

/** Makes near-white pixels transparent, leaving the ink's alpha intact. */
export function removeWhiteBackground(
  image: RgbaImage,
  threshold = 245,
): RgbaImage {
  const data = new Uint8ClampedArray(image.data);
  for (let offset = 0; offset < data.length; offset += 4) {
    if (
      data[offset] >= threshold &&
      data[offset + 1] >= threshold &&
      data[offset + 2] >= threshold
    ) {
      data[offset + 3] = 0;
    }
  }
  return { width: image.width, height: image.height, data };
}

/**
 * Crops fully transparent margins, keeping `padding` transparent pixels
 * around the ink. Returns null when nothing is visible.
 */
export function trimTransparent(image: RgbaImage, padding = 8): RgbaImage | null {
  const { width, height, data } = image;
  let left = width;
  let top = height;
  let right = -1;
  let bottom = -1;
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      if (data[(y * width + x) * 4 + 3] > 8) {
        left = Math.min(left, x);
        top = Math.min(top, y);
        right = Math.max(right, x);
        bottom = Math.max(bottom, y);
      }
    }
  }
  if (right < left || bottom < top) {
    return null;
  }
  const x0 = Math.max(0, left - padding);
  const y0 = Math.max(0, top - padding);
  const x1 = Math.min(width, right + padding + 1);
  const y1 = Math.min(height, bottom + padding + 1);
  const cropWidth = x1 - x0;
  const cropHeight = y1 - y0;
  const cropped = new Uint8ClampedArray(cropWidth * cropHeight * 4);
  for (let y = 0; y < cropHeight; y += 1) {
    const source = ((y0 + y) * width + x0) * 4;
    cropped.set(data.subarray(source, source + cropWidth * 4), y * cropWidth * 4);
  }
  return { width: cropWidth, height: cropHeight, data: cropped };
}

export interface ProcessedUpload {
  image: RgbaImage;
  removedBackground: boolean;
  trimmed: boolean;
}

/**
 * Applies the upload tab's background choice. Without removal the original
 * pixels (including an opaque JPEG background) are kept unchanged.
 */
export function processUploadedImage(
  image: RgbaImage,
  removeBackground: boolean,
): ProcessedUpload {
  if (!removeBackground) {
    return { image, removedBackground: false, trimmed: false };
  }
  const cleared = removeWhiteBackground(image);
  const trimmed = trimTransparent(cleared);
  if (trimmed) {
    return { image: trimmed, removedBackground: true, trimmed: true };
  }
  return { image: cleared, removedBackground: true, trimmed: false };
}

/** Encodes RGBA pixels as a transparent PNG data URL and base64 payload. */
export function encodePng(image: RgbaImage): CanvasImage | null {
  const canvas = document.createElement("canvas");
  canvas.width = image.width;
  canvas.height = image.height;
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    return null;
  }
  const pixels = ctx.createImageData(image.width, image.height);
  pixels.data.set(image.data);
  ctx.putImageData(pixels, 0, 0);
  const dataUrl = canvas.toDataURL("image/png");
  const base64 = pngDataUrlToBase64(dataUrl);
  return base64 ? { dataUrl, base64 } : null;
}

/** The two image encodings the upload tab accepts. */
export type UploadedFormat = "png" | "jpeg";

/** Detects PNG or JPEG from the file's magic bytes, not its name. */
export async function uploadedFormat(file: File): Promise<UploadedFormat | null> {
  const head = new Uint8Array(await file.slice(0, 8).arrayBuffer());
  if (
    head.length >= 8 &&
    head[0] === 0x89 &&
    head[1] === 0x50 &&
    head[2] === 0x4e &&
    head[3] === 0x47
  ) {
    return "png";
  }
  if (head.length >= 3 && head[0] === 0xff && head[1] === 0xd8 && head[2] === 0xff) {
    return "jpeg";
  }
  return null;
}

/**
 * The upload tab's pre-decode validation, with the same limits and Chinese
 * copy as the image insertion tool. `null` means the file may be decoded.
 */
export async function uploadedFileError(file: File): Promise<string | null> {
  if (file.size > SIGNATURE_MAX_FILE_BYTES) {
    return "图片过大：文件不能超过 25 MB，像素不能超过 2000 万。";
  }
  if ((await uploadedFormat(file)) === null) {
    return "仅支持 PNG 和 JPG 格式的图片。";
  }
  return null;
}

/** Decodes an uploaded file into RGBA pixels without touching the document. */
export async function decodeUploadedImage(file: File): Promise<RgbaImage> {
  const image = await loadImageElement(file);
  const width = image.naturalWidth || image.width;
  const height = image.naturalHeight || image.height;
  if (width === 0 || height === 0) {
    throw new Error("the image has no pixels");
  }
  if (width * height > SIGNATURE_MAX_PIXELS) {
    throw new Error("too many pixels");
  }
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    throw new Error("canvas is unavailable");
  }
  ctx.drawImage(image, 0, 0);
  return ctx.getImageData(0, 0, width, height);
}

async function loadImageElement(file: File): Promise<HTMLImageElement> {
  if (typeof URL.createObjectURL === "function") {
    const url = URL.createObjectURL(file);
    try {
      return await new Promise<HTMLImageElement>((resolve, reject) => {
        const image = new Image();
        image.onload = () => resolve(image);
        image.onerror = () => reject(new Error("the image could not be decoded"));
        image.src = url;
      });
    } finally {
      URL.revokeObjectURL(url);
    }
  }
  const dataUrl = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error ?? new Error("could not read the file"));
    reader.readAsDataURL(file);
  });
  return await new Promise<HTMLImageElement>((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error("the image could not be decoded"));
    image.src = dataUrl;
  });
}
